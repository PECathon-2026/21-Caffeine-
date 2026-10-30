//! Minimal incremental Server-Sent-Events parser.
//!
//! Feed raw bytes as they arrive off the wire (chunks may split anywhere,
//! including mid-line or mid-UTF-8-sequence); complete events are returned as
//! soon as their terminating blank line has been seen. Handles `\n` and
//! `\r\n` line endings, `event:`/`data:` fields (multiple `data:` lines are
//! joined with `\n` per the SSE spec), and ignores comments and other fields.

/// One parsed SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// The `event:` field, if present.
    pub event: Option<String>,
    /// All `data:` lines joined with `\n`.
    pub data: String,
}

/// Incremental parser; owns a byte buffer across `feed` calls.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a chunk and return every event completed by it.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((event_end, rest_start)) = find_event_boundary(&self.buf) {
            let raw: Vec<u8> = self.buf[..event_end].to_vec();
            self.buf.drain(..rest_start);
            if let Some(ev) = parse_event(&raw) {
                out.push(ev);
            }
        }
        out
    }
}

/// Find the first blank line terminating an event. Returns
/// `(end_of_event_bytes, start_of_remainder)`.
fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            // A following "\n" or "\r\n" makes the next line blank.
            if buf.get(i + 1) == Some(&b'\n') {
                return Some((i, i + 2));
            }
            if buf.get(i + 1) == Some(&b'\r') && buf.get(i + 2) == Some(&b'\n') {
                return Some((i, i + 3));
            }
        }
        i += 1;
    }
    None
}

/// Parse one event's raw bytes (without the trailing blank line).
fn parse_event(raw: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(raw);
    let mut event: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with(':') {
            continue; // comment / keep-alive
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = Some(v.strip_prefix(' ').unwrap_or(v).to_string());
        } else if let Some(v) = line.strip_prefix("data:") {
            data_lines.push(v.strip_prefix(' ').unwrap_or(v));
        }
        // other fields (id:, retry:) are intentionally ignored
    }
    if event.is_none() && data_lines.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data_lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            event: event.map(str::to_string),
            data: data.to_string(),
        }
    }

    #[test]
    fn single_event_single_chunk() {
        let mut p = SseParser::new();
        let got = p.feed(b"event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(got, vec![ev(Some("message_start"), "{\"a\":1}")]);
    }

    #[test]
    fn chunk_split_mid_line() {
        let mut p = SseParser::new();
        assert!(p.feed(b"eve").is_empty());
        assert!(p.feed(b"nt: delta\nda").is_empty());
        assert!(p.feed(b"ta: hel").is_empty());
        let got = p.feed(b"lo\n\n");
        assert_eq!(got, vec![ev(Some("delta"), "hello")]);
    }

    #[test]
    fn chunk_split_between_terminator_newlines() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: x\n").is_empty());
        let got = p.feed(b"\n");
        assert_eq!(got, vec![ev(None, "x")]);
    }

    #[test]
    fn multiple_events_one_chunk() {
        let mut p = SseParser::new();
        let got = p.feed(b"data: one\n\nevent: e2\ndata: two\n\ndata: partial");
        assert_eq!(got, vec![ev(None, "one"), ev(Some("e2"), "two")]);
        let got = p.feed(b"\n\n");
        assert_eq!(got, vec![ev(None, "partial")]);
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        let got = p.feed(b"event: stop\r\ndata: {}\r\n\r\n");
        assert_eq!(got, vec![ev(Some("stop"), "{}")]);
    }

    #[test]
    fn multi_data_lines_join_with_newline() {
        let mut p = SseParser::new();
        let got = p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(got, vec![ev(None, "line1\nline2")]);
    }

    #[test]
    fn comments_and_unknown_fields_ignored() {
        let mut p = SseParser::new();
        let got = p.feed(b": keep-alive\nid: 7\nretry: 100\ndata: ok\n\n");
        assert_eq!(got, vec![ev(None, "ok")]);
        // an event consisting solely of a comment produces nothing
        assert!(p.feed(b": ping\n\n").is_empty());
    }

    #[test]
    fn no_space_after_colon() {
        let mut p = SseParser::new();
        let got = p.feed(b"data:[DONE]\n\n");
        assert_eq!(got, vec![ev(None, "[DONE]")]);
    }

    #[test]
    fn utf8_split_across_chunks() {
        let mut p = SseParser::new();
        let text = "data: héllo\n\n".as_bytes();
        // split inside the two-byte 'é'
        let split = text.iter().position(|&b| b == 0xc3).map(|i| i + 1);
        let split = match split {
            Some(s) => s,
            None => panic!("expected multibyte char"),
        };
        assert!(p.feed(&text[..split]).is_empty());
        let got = p.feed(&text[split..]);
        assert_eq!(got, vec![ev(None, "héllo")]);
    }
}
