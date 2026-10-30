//! Pure incremental Server-Sent-Events parser.
//!
//! Feed arbitrary byte chunks with [`SseParser::push`]; complete frames come
//! back in order. No I/O, no allocatons beyond the frames themselves — fully
//! unit-testable across chunk boundaries.

/// One dispatched SSE frame (`id:` / `event:` / `data:` fields between blank
/// lines). `data` joins multiple `data:` lines with `\n` per the SSE spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

/// Incremental parser state. `push` buffers partial lines so chunk boundaries
/// may fall anywhere (mid-line, mid-UTF-8 even — bytes are buffered raw).
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    id: Option<String>,
    event: Option<String>,
    data: Option<String>,
    /// Last dispatched (or seen) id — carried across frames per SSE semantics.
    last_id: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recent `id:` observed (for `Last-Event-ID` on reconnect).
    pub fn last_id(&self) -> Option<&str> {
        self.last_id.as_deref()
    }

    /// Feed a chunk; returns every frame completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            if let Some(frame) = self.line(&line) {
                out.push(frame);
            }
        }
        out
    }

    fn line(&mut self, line: &str) -> Option<SseFrame> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment / heartbeat
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => {
                let data = self.data.get_or_insert_with(String::new);
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            "event" => self.event = Some(value.to_string()),
            "id" if !value.contains('\0') => {
                self.id = Some(value.to_string());
                self.last_id = Some(value.to_string());
            }
            _ => {} // "retry", ids with NUL, and unknown fields ignored
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseFrame> {
        let data = self.data.take();
        let event = self.event.take();
        let id = self.id.take();
        // Per spec: no data buffered → nothing dispatched.
        let data = data?;
        Some(SseFrame { id, event, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_frame() {
        let mut p = SseParser::new();
        let frames = p.push(b"id: 7\nevent: event\ndata: {\"a\":1}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id.as_deref(), Some("7"));
        assert_eq!(frames[0].event.as_deref(), Some("event"));
        assert_eq!(frames[0].data, "{\"a\":1}");
    }

    #[test]
    fn survives_arbitrary_chunk_boundaries() {
        let input = b"id: 42\ndata: hello\ndata: world\n\nid: 43\ndata: bye\n\n";
        // Split at every possible boundary and assert identical output.
        for split in 0..input.len() {
            let mut p = SseParser::new();
            let mut frames = p.push(&input[..split]);
            frames.extend(p.push(&input[split..]));
            assert_eq!(frames.len(), 2, "split at {split}");
            assert_eq!(frames[0].data, "hello\nworld");
            assert_eq!(frames[0].id.as_deref(), Some("42"));
            assert_eq!(frames[1].data, "bye");
            assert_eq!(frames[1].id.as_deref(), Some("43"));
        }
    }

    #[test]
    fn byte_at_a_time() {
        let input = b"data: a\n\ndata: b\n\n";
        let mut p = SseParser::new();
        let mut frames = Vec::new();
        for b in input {
            frames.extend(p.push(&[*b]));
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "a");
        assert_eq!(frames[1].data, "b");
    }

    #[test]
    fn ignores_comments_and_blank_only_frames() {
        let mut p = SseParser::new();
        let frames = p.push(b": heartbeat\n\n: another\n\ndata: x\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "x");
    }

    #[test]
    fn tracks_last_event_id_across_frames() {
        let mut p = SseParser::new();
        p.push(b"id: 5\ndata: one\n\ndata: two\n\n");
        assert_eq!(p.last_id(), Some("5"));
    }

    #[test]
    fn handles_crlf_lines() {
        let mut p = SseParser::new();
        let frames = p.push(b"id: 9\r\ndata: hi\r\n\r\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "hi");
        assert_eq!(frames[0].id.as_deref(), Some("9"));
    }

    #[test]
    fn field_without_colon_is_field_name_with_empty_value() {
        let mut p = SseParser::new();
        // "data" alone → empty data line; frame carries "".
        let frames = p.push(b"data\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "");
    }
}
