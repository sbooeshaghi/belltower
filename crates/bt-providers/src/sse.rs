#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        self.buffer = self.buffer.replace("\r\n", "\n");
        let mut events = Vec::new();
        while let Some(boundary) = self.buffer.find("\n\n") {
            let frame = self.buffer[..boundary].to_owned();
            self.buffer = self.buffer[boundary + 2..].to_owned();
            if frame.is_empty() {
                continue;
            }
            let mut event_type = None;
            let mut data = Vec::new();
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event_type = Some(value.trim().to_owned());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.trim().to_owned());
                }
            }

            if !data.is_empty() {
                events.push(SseEvent {
                    event: event_type,
                    data: data.join("\n"),
                });
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn parser_handles_split_events() {
        let mut parser = SseParser::new();
        let first = parser.push(b"event: message\ndata: {\"a\":");
        assert!(first.is_empty());
        let second = parser.push(b"1}\n\n");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].event.as_deref(), Some("message"));
        assert_eq!(second[0].data, "{\"a\":1}");
    }

    #[test]
    fn parser_handles_multiple_complete_frames_and_partial_tail() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\ndata: {\"c\":");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[1].data, "{\"b\":2}");

        let tail = parser.push(b"3}\n\n");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].data, "{\"c\":3}");
    }
}
