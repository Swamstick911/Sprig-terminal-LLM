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
///
/// Control frames are recognised by their top-level `"type"` field, so reply
/// text that merely *mentions* `message_stop` or `error` is never mistaken for
/// a control frame.
pub fn process_line(line: &str, delta: &mut String<256>) -> SseOut {
    let line = line.trim_end_matches('\r');
    let rest = match line.strip_prefix("data:") {
        Some(r) => r.trim_start(),
        None => return SseOut::None, // `event:` lines, comments, blanks
    };
    if rest == "[DONE]" {
        return SseOut::Stop; // OpenAI-style terminator
    }
    match json::frame_type(rest) {
        "error" => SseOut::Error,
        "message_stop" => SseOut::Stop,
        // content_block_delta (and any frame without a recognised top-level
        // type, e.g. abbreviated test payloads) may carry a text delta.
        _ => {
            if json::extract_text_delta(rest, delta) {
                SseOut::Delta
            } else {
                SseOut::None
            }
        }
    }
}

/// Classify one OpenAI / OpenRouter SSE line. On [`SseOut::Delta`], `delta`
/// holds the new `choices[0].delta.content` text. The stream ends with the
/// literal `data: [DONE]` line.
pub fn process_openai_line(line: &str, delta: &mut String<256>) -> SseOut {
    let line = line.trim_end_matches('\r');
    let rest = match line.strip_prefix("data:") {
        Some(r) => r.trim_start(),
        None => return SseOut::None,
    };
    if rest == "[DONE]" {
        return SseOut::Stop;
    }
    // Top-level `"error":` only — never matches the word "error" inside content,
    // whose quotes are escaped.
    if rest.contains("\"error\":") {
        return SseOut::Error;
    }
    if json::extract_openai_delta(rest, delta) {
        return SseOut::Delta;
    }
    SseOut::None
}

/// For an OpenAI / OpenRouter `data:` line, return `usage.total_tokens` if the
/// chunk carries a usage object (only the final chunk does, and only when the
/// request opted in via `stream_options.include_usage`). Returns `None` for
/// `event:`/comment/blank lines and for ordinary delta chunks.
pub fn usage_total(line: &str) -> Option<u32> {
    let line = line.trim_end_matches('\r');
    let rest = line.strip_prefix("data:")?.trim_start();
    json::extract_total_tokens(rest)
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
        let line = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\r";
        assert_eq!(process_line(line, &mut d), SseOut::Delta);
        assert_eq!(d.as_str(), "ok");
    }

    #[test]
    fn openai_delta_done_and_error() {
        let mut d: String<256> = String::new();
        assert_eq!(
            process_openai_line(r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#, &mut d),
            SseOut::Delta
        );
        assert_eq!(d.as_str(), "Hi");
        assert_eq!(process_openai_line("data: [DONE]", &mut d), SseOut::Stop);
        assert_eq!(
            process_openai_line(r#"data: {"error":{"message":"bad"}}"#, &mut d),
            SseOut::Error
        );
    }

    #[test]
    fn openai_content_saying_error_is_a_delta_not_an_error() {
        let mut d: String<256> = String::new();
        let line = r#"data: {"choices":[{"delta":{"content":"an error occurred"}}]}"#;
        assert_eq!(process_openai_line(line, &mut d), SseOut::Delta);
        assert_eq!(d.as_str(), "an error occurred");
    }

    #[test]
    fn usage_total_reads_final_chunk() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":9,"total_tokens":12}}"#;
        assert_eq!(usage_total(line), Some(12));
        // Normal delta lines and non-data lines carry no usage.
        assert_eq!(
            usage_total(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#),
            None
        );
        assert_eq!(usage_total("event: message"), None);
    }

    #[test]
    fn reply_text_mentioning_control_types_is_not_misclassified() {
        // The model's own output talks about the message_stop event. Because the
        // quotes inside the delta text are escaped, this is a normal text delta.
        let mut d: String<256> = String::new();
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"emit \"type\":\"message_stop\" to end"}}"#;
        assert_eq!(process_line(line, &mut d), SseOut::Delta);
        assert_eq!(d.as_str(), "emit \"type\":\"message_stop\" to end");
    }
}
