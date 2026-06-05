//! Minimal, allocation-free JSON helpers for the streaming path.
//!
//! Two jobs:
//! * [`escape_into`] — escape a user prompt so it can be embedded in a request
//!   body string.
//! * [`extract_text_delta`] — pull `delta.text` out of an Anthropic
//!   `content_block_delta` SSE payload, decoding JSON escapes incrementally.
//!
//! These avoid a full JSON parser to keep code size and RAM down; the shapes we
//! consume are fixed and small.

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

/// Decode a JSON string body (the characters *after* the opening quote) into
/// `out`, stopping at the unescaped closing quote. Returns `true` on success.
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
                    'u' => {
                        let mut cp: u32 = 0;
                        for _ in 0..4 {
                            let h = match it.next() {
                                Some(c) => c,
                                None => return false,
                            };
                            let v = match h.to_digit(16) {
                                Some(v) => v,
                                None => return false,
                            };
                            cp = cp * 16 + v;
                        }
                        core::char::from_u32(cp).unwrap_or('\u{FFFD}')
                    }
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
    fn ignores_non_text_delta() {
        let data = r#"{"delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#;
        let mut out: String<256> = String::new();
        assert!(!extract_text_delta(data, &mut out));
    }
}
