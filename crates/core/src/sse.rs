//! Server-Sent Events line handling for the streamed LLM response.
//!
//! The network layer de-chunks the HTTP/1.1 body and feeds complete lines here.
//! [`process_line`] classifies each line and, for a text delta, decodes the new
//! text into the caller's buffer. Peak extra RAM is one delta string — the full
//! response is never buffered.

use crate::json;
use heapless::String;

/// Result of classifying one SSE line.
#[derive(Debug, PartialEq, Eq)]
pub enum SseOut {
    /// Not a meaningful data line (comment, `event:` line, blank, other event).
    None,
    /// A text delta was decoded into the caller's buffer.
    Delta,
    /// Terminal: the stream finished (`message_stop` / `[DONE]`).
    Stop,
    /// An error event was received.
    Error,
}

/// Classify one SSE line. On [`SseOut::Delta`], `delta` holds the new text.
pub fn process_line(line: &str, delta: &mut String<256>) -> SseOut {
    let line = line.trim_end_matches('\r');
    let rest = match line.strip_prefix("data:") {
        Some(r) => r.trim_start(),
        None => return SseOut::None, // `event:` lines, comments, blanks
    };
    if rest == "[DONE]" {
        return SseOut::Stop;
    }
    if rest.contains("\"type\":\"error\"") {
        return SseOut::Error;
    }
    if rest.contains("\"type\":\"message_stop\"") {
        return SseOut::Stop;
    }
    if json::extract_text_delta(rest, delta) {
        return SseOut::Delta;
    }
    SseOut::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_delta_line() {
        let mut d: String<256> = String::new();
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(process_line(line, &mut d), SseOut::Delta);
        assert_eq!(d.as_str(), "Hi");
    }

    #[test]
    fn event_lines_are_ignored() {
        let mut d: String<256> = String::new();
        assert_eq!(process_line("event: content_block_delta", &mut d), SseOut::None);
        assert_eq!(process_line("", &mut d), SseOut::None);
        assert_eq!(process_line(": ping", &mut d), SseOut::None);
    }

    #[test]
    fn detects_stop() {
        let mut d: String<256> = String::new();
        assert_eq!(
            process_line(r#"data: {"type":"message_stop"}"#, &mut d),
            SseOut::Stop
        );
        assert_eq!(process_line("data: [DONE]", &mut d), SseOut::Stop);
    }

    #[test]
    fn detects_error() {
        let mut d: String<256> = String::new();
        let line = r#"data: {"type":"error","error":{"type":"overloaded_error","message":"x"}}"#;
        assert_eq!(process_line(line, &mut d), SseOut::Error);
    }

    #[test]
    fn handles_trailing_cr() {
        let mut d: String<256> = String::new();
        let line = "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\r";
        assert_eq!(process_line(line, &mut d), SseOut::Delta);
        assert_eq!(d.as_str(), "ok");
    }
}
