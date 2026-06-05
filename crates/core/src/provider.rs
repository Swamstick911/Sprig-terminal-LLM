//! LLM provider abstraction.
//!
//! The terminal speaks raw HTTPS (there is no official Anthropic SDK for Rust,
//! and none would be `no_std`). A [`LlmProvider`] knows its host/path, how to
//! build a streaming request body, and how to pull a text delta out of one SSE
//! `data:` payload. Swapping providers (Claude ↔ OpenAI) is one impl.

use crate::json;
use core::fmt::Write;
use heapless::String;

/// A hosted LLM the terminal can stream from.
pub trait LlmProvider {
    /// API host, e.g. `api.anthropic.com`.
    fn host(&self) -> &str;
    /// Request path, e.g. `/v1/messages`.
    fn path(&self) -> &str;
    /// Build the JSON request body for a streaming chat completion.
    fn build_body<const N: usize>(&self, prompt: &str, out: &mut String<N>) -> Result<(), ()>;
    /// Extract a text delta from one SSE `data:` payload; `true` if one was found.
    fn extract_delta(&self, data_json: &str, out: &mut String<256>) -> bool;
}

/// Anthropic Claude (Messages API). Default provider.
pub struct Claude<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
}

impl<'a> Claude<'a> {
    /// New Claude provider with a sensible default `max_tokens`.
    pub const fn new(model: &'a str) -> Self {
        Self {
            model,
            max_tokens: 1024,
        }
    }

    /// Required request headers (name, value). The API key is supplied
    /// separately by the network layer so it never lives in this struct.
    pub const ANTHROPIC_VERSION: &'static str = "2023-06-01";
}

impl LlmProvider for Claude<'_> {
    fn host(&self) -> &str {
        "api.anthropic.com"
    }

    fn path(&self) -> &str {
        "/v1/messages"
    }

    fn build_body<const N: usize>(&self, prompt: &str, out: &mut String<N>) -> Result<(), ()> {
        out.clear();
        out.push_str("{\"model\":\"").map_err(|_| ())?;
        out.push_str(self.model).map_err(|_| ())?;
        write!(
            out,
            "\",\"max_tokens\":{},\"stream\":true,\"messages\":[{{\"role\":\"user\",\"content\":\"",
            self.max_tokens
        )
        .map_err(|_| ())?;
        json::escape_into(prompt, out)?;
        out.push_str("\"}]}").map_err(|_| ())?;
        Ok(())
    }

    fn extract_delta(&self, data_json: &str, out: &mut String<256>) -> bool {
        json::extract_text_delta(data_json, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_valid_claude_body() {
        let c = Claude::new("claude-opus-4-8");
        let mut body: String<512> = String::new();
        c.build_body("hi there", &mut body).unwrap();
        assert_eq!(
            body.as_str(),
            r#"{"model":"claude-opus-4-8","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"hi there"}]}"#
        );
    }

    #[test]
    fn escapes_prompt_in_body() {
        let c = Claude::new("claude-haiku-4-5");
        let mut body: String<512> = String::new();
        c.build_body("say \"hi\"\nnow", &mut body).unwrap();
        assert!(body.as_str().contains(r#""content":"say \"hi\"\nnow""#));
        assert!(body.as_str().contains(r#""model":"claude-haiku-4-5""#));
    }

    #[test]
    fn host_and_path() {
        let c = Claude::new("claude-opus-4-8");
        assert_eq!(c.host(), "api.anthropic.com");
        assert_eq!(c.path(), "/v1/messages");
    }
}
