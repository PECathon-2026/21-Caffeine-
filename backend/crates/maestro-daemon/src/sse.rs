//! GET /api/events — replay from a cursor, then follow the live event bus.
//!
//! Frames are `event: message` with `id: <seq>` and the [`EventRecord`] JSON
//! (camelCase) as data. A `: ping` comment is emitted every 15 seconds by a
//! ticker owned by the per-connection feeder task; the task itself is owned
//! by the response stream and aborted when the client disconnects. Broadcast
//! `Lagged` errors are tolerated by re-replaying from the last sent seq.

use crate::http::AppState;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use futures::Stream;
use maestro_store::Store;
use maestro_types::event::EventRecord;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PING_PERIOD: Duration = Duration::from_secs(15);
/// Per-connection frame buffer; the feeder awaits when a client is slow.
const CHANNEL_CAPACITY: usize = 256;
/// Replay page size: the store lock is taken per page instead of holding it
/// (and one giant Vec) for the entire log.
const REPLAY_BATCH: usize = 1000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventsQuery {
    #[serde(default)]
    mission_id: Option<String>,
    #[serde(default)]
    after: Option<i64>,
}

pub(crate) async fn events_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Cursor precedence: Last-Event-ID, then explicit ?after=, then 0. The
    // header only appears on reconnect and always carries the true resume
    // point; a standard EventSource resends its original URL, so a stale
    // ?after= must never win over it (SPEC: "Last-Event-ID honored").
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let cursor = last_event_id.or(query.after).unwrap_or(0);

    let (tx, rx) = mpsc::channel::<Event>(CHANNEL_CAPACITY);
    let handle = tokio::spawn(feed(
        state.store.clone(),
        state.shutdown.clone(),
        query.mission_id,
        cursor,
        tx,
    ));
    Sse::new(EventFeed {
        rx,
        _feeder: AbortOnDrop(handle),
    })
}

fn frame(record: &EventRecord) -> Result<Event, serde_json::Error> {
    Ok(Event::default()
        .id(record.seq.to_string())
        .event("message")
        .data(serde_json::to_string(record)?))
}

/// Send one frame, racing daemon shutdown. A client that stopped reading
/// (stalled TCP peer) fills the channel and parks the send indefinitely; the
/// send must still observe the shutdown token or the SSE body never ends and
/// graceful shutdown hangs forever. Returns false when the feeder should
/// exit (client gone or shutting down).
async fn send_or_close(
    tx: &mpsc::Sender<Event>,
    shutdown: &CancellationToken,
    event: Event,
) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => false,
        sent = tx.send(event) => sent.is_ok(),
    }
}

/// Feeder loop: subscribe first (so nothing is missed), replay from the
/// cursor, then follow live. Exits on client drop, daemon shutdown, or a
/// closed bus. `Lagged` jumps back to the replay step.
async fn feed(
    store: Store,
    shutdown: CancellationToken,
    mission_id: Option<String>,
    mut cursor: i64,
    tx: mpsc::Sender<Event>,
) {
    let mut bus = store.subscribe();
    let mut ping = tokio::time::interval(PING_PERIOD);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    'replay: loop {
        // Page through the backlog: bounded memory, and the store mutex is
        // released between batches so writers are never starved by a replay.
        loop {
            let batch = match store.events_page(cursor, mission_id.as_deref(), REPLAY_BATCH) {
                Ok(batch) => batch,
                Err(e) => {
                    tracing::warn!(error = %e, "sse replay failed; closing stream");
                    return;
                }
            };
            let more = batch.len() == REPLAY_BATCH;
            for record in &batch {
                cursor = cursor.max(record.seq);
                let Ok(event) = frame(record) else { continue };
                if !send_or_close(&tx, &shutdown, event).await {
                    return; // client went away or daemon shutting down
                }
            }
            if !more {
                break;
            }
            tokio::task::yield_now().await;
        }

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = ping.tick() => {
                    if !send_or_close(&tx, &shutdown, Event::default().comment("ping")).await {
                        return;
                    }
                }
                received = bus.recv() => match received {
                    Ok(record) => {
                        if record.seq <= cursor {
                            continue;
                        }
                        if let Some(m) = &mission_id {
                            if &record.mission_id != m {
                                continue;
                            }
                        }
                        cursor = record.seq;
                        let Ok(event) = frame(&record) else { continue };
                        if !send_or_close(&tx, &shutdown, event).await {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "sse subscriber lagged; re-replaying");
                        continue 'replay;
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }
}

/// Owns the feeder task; aborts it when the connection stream drops.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The response body stream: frames from the feeder, ending when it stops.
struct EventFeed {
    rx: mpsc::Receiver<Event>,
    _feeder: AbortOnDrop,
}

impl Stream for EventFeed {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|opt| opt.map(Ok))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding (wave 4): a feeder parked in `tx.send` on a full channel (a
    /// client that stopped reading) must still observe daemon shutdown —
    /// otherwise the SSE body never ends and axum's graceful shutdown waits
    /// on the connection forever.
    #[tokio::test(flavor = "multi_thread")]
    async fn parked_feeder_exits_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mission = store.create_mission("stalled client").unwrap();
        for i in 0..(CHANNEL_CAPACITY * 2) {
            store
                .append_event(
                    &mission.id,
                    None,
                    "bench.tick",
                    serde_json::json!({ "i": i }),
                )
                .unwrap();
        }

        // The receiver is held but never read: the feeder fills the channel
        // and parks in `send` mid-replay, exactly like a stalled TCP peer.
        let shutdown = CancellationToken::new();
        let (tx, _rx) = mpsc::channel::<Event>(CHANNEL_CAPACITY);
        let feeder = tokio::spawn(feed(store, shutdown.clone(), None, 0, tx));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !feeder.is_finished(),
            "feeder should be parked on the full channel"
        );

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), feeder)
            .await
            .expect("feeder must exit on shutdown while parked in send")
            .unwrap();
    }
}
