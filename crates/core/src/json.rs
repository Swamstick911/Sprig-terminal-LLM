//! Minimal, allocation-free JSON helpers for the streaming path.
//!
//! Two jobs:
//! * [`escape_into`] — escape a user prompt so it can be embedded in a request
//!   body string.
//! * [`extract_text_delta`] — pull `delta.text` out of an Anthropic
//!   `content_block_delta` SSE payload, decoding JSON escapes incrementally.
//!
//! These avoid a full JSON parser to keep code size and RAM down; the shapes we
//! consume are fixed and small. Structural matching is safe because any `"`
//! inside a JSON string value is escaped as `\"`, so an unescaped `"key"` only
//! ever appears as a real key.

use heapless::String;

fn hex_nibble(n: u8) -> u8 {
    if n < 10 {
        b'0' + n
    } else {
        b'a' + (n - 10)
    }
}

fn push_u_escape<const N: usize>(out: &mut String<N>, cp: u32) -> Result<(), ()> {
    out.push_str("\\u").map_err(|_| ())?;
    for shift in [12u32, 8, 4, 0] {
        let nib = ((cp >> shift) & 0xF) as u8;
        out.push(hex_nibble(nib) as char).map_err(|_| ())?;
    }
    Ok(())
}

/// Append `s` to `out` with JSON string escaping (no surrounding quotes).
/// Returns `Err` if `out` runs out of capacity.
pub fn escape_into<const N: usize>(s: &str, out: &mut String<N>) -> Result<(), ()> {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\"").map_err(|_| ())?,
            '\\' => out.push_str("\\\\").map_err(|_| ())?,
            '\n' => out.push_str("\\n").map_err(|_| ())?,
            '\r' => out.push_str("\\r").map_err(|_| ())?,
            '\t' => out.push_str("\\t").map_err(|_| ())?,
            c if (c as u32) < 0x20 => push_u_escape(out, c as u32)?,
            c => out.push(c).map_err(|_| ())?,
        }
    }
    Ok(())
}

/// Read exactly four hex digits from `it`, returning the value.
fn read_hex4(it: &mut core::str::Chars<'_>) -> Option<u32> {
    let mut cp = 0u32;
    for _ in 0..4 {
        cp = cp * 16 + it.next()?.to_digit(16)?;
    }
    Some(cp)
}

/// Decode a JSON string body (the characters *after* the opening quote) into
/// `out`, stopping at the unescaped closing quote. Returns `true` on success.
///
/// Handles the standard escapes plus `\uXXXX`, including UTF-16 surrogate pairs
/// (`😀` → 😀) so astral-plane characters survive.
pub fn decode_string_body<const N: usize>(s: &str, out: &mut String<N>) -> bool {
    let mut it = s.chars();
    loop {
        let c = match it.next() {
            Some(c) => c,
            None => return false, // ran out before closing quote
        };
        match c {
            '"' => return true,
            '\\' => {
                let e = match it.next() {
                    Some(c) => c,
                    None => return false,
                };
                let decoded = match e {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'b' => '\u{08}',
                    'f' => '\u{0C}',
                    'u' => match decode_u_escape(&mut it) {
                        Some(ch) => ch,
                        None => return false,
                    },
                    other => other,
                };
                if out.push(decoded).is_err() {
                    return false;
                }
            }
            c => {
                if out.push(c).is_err() {
                    return false;
                }
            }
        }
    }
}

/// Decode the four hex digits after a `\u`, combining a surrogate pair if the
/// first unit is a high surrogate. The leading `\u` has already been consumed.
fn decode_u_escape(it: &mut core::str::Chars<'_>) -> Option<char> {
    let hi = read_hex4(it)?;
    if (0xD800..=0xDBFF).contains(&hi) {
        // High surrogate: expect a following \uXXXX low surrogate.
        if it.next()? != '\\' || it.next()? != 'u' {
            return Some('\u{FFFD}');
        }
        let lo = read_hex4(it)?;
        if (0xDC00..=0xDFFF).contains(&lo) {
            let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            Some(core::char::from_u32(cp).unwrap_or('\u{FFFD}'))
        } else {
            Some('\u{FFFD}')
        }
    } else {
        Some(core::char::from_u32(hi).unwrap_or('\u{FFFD}'))
    }
}

/// Value of the top-level `"type"` field of an SSE frame, or `""` if absent.
///
/// The first `"type"` in an Anthropic frame is always the top-level event type
/// (`content_block_delta`, `message_stop`, `error`, …); these tokens contain no
/// escapes, so a plain slice is enough.
pub fn frame_type(json: &str) -> &str {
    let after = match json.find("\"type\"") {
        Some(i) => &json[i + "\"type\"".len()..],
        None => return "",
    };
    let body = match after.find('"') {
        Some(i) => &after[i + 1..],
        None => return "",
    };
    match body.find('"') {
        Some(i) => &body[..i],
        None => "",
    }
}

/// Extract `delta.text` from an Anthropic `content_block_delta` payload.
///
/// Returns `true` and fills `out` only when the payload is a `text_delta`.
/// Other delta types (e.g. `input_json_delta`) and non-delta events return
/// `false`.
pub fn extract_text_delta<const N: usize>(json: &str, out: &mut String<N>) -> bool {
    let after = match json.find("\"text_delta\"") {
        Some(i) => &json[i..],
        None => return false,
    };
    let key = match after.find("\"text\"") {
        Some(i) => &after[i + "\"text\"".len()..],
        None => return false,
    };
    let body = match key.find('"') {
        Some(i) => &key[i + 1..],
        None => return false,
    };
    out.clear();
    decode_string_body(body, out)
}

/// Extract `choices[0].delta.content` from an OpenAI / OpenRouter streaming
/// chunk. Returns `true` and fills `out` when the chunk carries text content;
/// `false` for role-only / `content: null` / finish chunks.
pub fn extract_openai_delta<const N: usize>(json: &str, out: &mut String<N>) -> bool {
    let after = match json.find("\"delta\"") {
        Some(i) => &json[i..],
        None => return false,
    };
    let key = match after.find("\"content\"") {
        Some(i) => &after[i + "\"content\"".len()..],
        None => return false,
    };
    // Skip the `:` and whitespace; content must be a string (not `null`).
    let rest = key.trim_start_matches([':', ' ', '\t']);
    if !rest.starts_with('"') {
        return false; // e.g. "content":null
    }
    out.clear();
    decode_string_body(&rest[1..], out)
}

/// Extract `usage.total_tokens` from an OpenAI / OpenRouter chunk.
///
/// OpenRouter only emits a `"usage"` object (in the final chunk) when the
/// request asked for it via `stream_options.include_usage`. Locates the
/// `"usage"` key, then the `"total_tokens"` integer inside it, and parses its
/// digits. Returns `None` when either key is absent (e.g. a normal delta chunk).
pub fn extract_total_tokens(json: &str) -> Option<u32> {
    let after = match json.find("\"usage\"") {
        Some(i) => &json[i + "\"usage\"".len()..],
        None => return None,
    };
    let key = match after.find("\"total_tokens\"") {
        Some(i) => &after[i + "\"total_tokens\"".len()..],
        None => return None,
    };
    // Skip the `:` and any whitespace, then read decimal digits.
    let digits = key.trim_start_matches([':', ' ', '\t']);
    let mut value: u32 = 0;
    let mut seen = false;
    for c in digits.chars() {
        match c.to_digit(10) {
            Some(d) => {
                value = value.saturating_mul(10).saturating_add(d);
                seen = true;
            }
            None => break,
        }
    }
    if seen {
        Some(value)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(s: &str) -> String<128> {
        let mut out = String::new();
        escape_into(s, &mut out).unwrap();
        out
    }

    #[test]
    fn escapes_specials() {
        assert_eq!(esc("a\"b\\c\nd\te").as_str(), "a\\\"b\\\\c\\nd\\te");
    }

    #[test]
    fn escapes_control_char_as_unicode() {
        assert_eq!(esc("\u{01}").as_str(), "\\u0001");
    }

    #[test]
    fn decodes_plain_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello, world"}}"#;
        let mut out: String<256> = String::new();
        assert!(extract_text_delta(data, &mut out));
        assert_eq!(out.as_str(), "Hello, world");
    }

    #[test]
    fn decodes_escapes_in_delta() {
        let data = r#"{"delta":{"type":"text_delta","text":"line1\nline2 \"q\" A"}}"#;
        let mut out: String<256> = String::new();
        assert!(extract_text_delta(data, &mut out));
        assert_eq!(out.as_str(), "line1\nline2 \"q\" A");
    }

    #[test]
    fn decodes_surrogate_pair() {
        // The escaped JSON contains the UTF-16 surrogate pair for U+1F600.
        let data = "{\"delta\":{\"type\":\"text_delta\",\"text\":\"hi \\uD83D\\uDE00!\"}}";
        let mut out: String<256> = String::new();
        assert!(extract_text_delta(data, &mut out));
        assert_eq!(out.as_str(), "hi \u{1F600}!");
    }

    #[test]
    fn decodes_bmp_unicode_escape() {
        let data = "{\"delta\":{\"type\":\"text_delta\",\"text\":\"caf\\u00e9\"}}";
        let mut out: String<256> = String::new();
        assert!(extract_text_delta(data, &mut out));
        assert_eq!(out.as_str(), "caf\u{e9}");
    }

    #[test]
    fn ignores_non_text_delta() {
        let data = r#"{"delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#;
        let mut out: String<256> = String::new();
        assert!(!extract_text_delta(data, &mut out));
    }

    #[test]
    fn extracts_openai_delta_content() {
        let data = r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}"#;
        let mut out: String<256> = String::new();
        assert!(extract_openai_delta(data, &mut out));
        assert_eq!(out.as_str(), "Hello");
    }

    #[test]
    fn openai_null_content_and_finish_yield_nothing() {
        let mut out: String<256> = String::new();
        assert!(!extract_openai_delta(
            r#"{"choices":[{"delta":{"role":"assistant","content":null}}]}"#,
            &mut out
        ));
        assert!(!extract_openai_delta(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut out
        ));
    }

    #[test]
    fn extracts_total_tokens_from_usage_chunk() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":30,"total_tokens":42}}"#;
        assert_eq!(extract_total_tokens(data), Some(42));
    }

    #[test]
    fn no_total_tokens_in_normal_delta() {
        let data = r#"{"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(extract_total_tokens(data), None);
        // A usage object without total_tokens also yields None.
        assert_eq!(
            extract_total_tokens(r#"{"usage":{"prompt_tokens":5}}"#),
            None
        );
    }

    #[test]
    fn frame_type_reads_top_level() {
        assert_eq!(
            frame_type(r#"{"type":"content_block_delta","delta":{"type":"text_delta"}}"#),
            "content_block_delta"
        );
        assert_eq!(frame_type(r#"{"type":"message_stop"}"#), "message_stop");
        assert_eq!(frame_type(r#"{"index":0}"#), "");
    }
}
