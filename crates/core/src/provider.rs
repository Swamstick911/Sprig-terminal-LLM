//! LLM provider abstraction.
//!
//! The terminal speaks raw HTTPS (there is no official Anthropic SDK for Rust,
//! and none would be `no_std`). A [`LlmProvider`] knows its host/path, how to
//! build a streaming request body, and how to pull a text delta out of one SSE
//! `data:` payload. Swapping providers (Claude ↔ OpenAI) is one impl.

use crate::json;
use core::fmt::Write;
use heapless::String;

/// A chat message role for multi-turn requests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// Append one `{"role":..,"content":..}` object into an OpenAI-style `messages`
/// array, with a leading comma after the first element.
fn write_msg<const N: usize>(
    out: &mut String<N>,
    role: &str,
    content: &str,
    first: &mut bool,
) -> Result<(), ()> {
    if !*first {
        out.push(',').map_err(|_| ())?;
    }
    *first = false;
    out.push_str("{\"role\":\"").map_err(|_| ())?;
    out.push_str(role).map_err(|_| ())?;
    out.push_str("\",\"content\":\"").map_err(|_| ())?;
    json::escape_into(content, out)?;
    out.push_str("\"}").map_err(|_| ())?;
    Ok(())
}

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

/// OpenRouter (OpenAI-compatible chat completions). Lets the terminal reach
/// many models — e.g. `deepseek/deepseek-chat` — through one endpoint. Auth is
/// `Authorization: Bearer <key>` (set by the network layer), not `x-api-key`.
pub struct OpenRouter<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
}

impl<'a> OpenRouter<'a> {
    pub const fn new(model: &'a str) -> Self {
        Self {
            model,
            max_tokens: 1024,
        }
    }

    /// Build a streaming chat-completions body from an optional system prompt
    /// and an ordered list of conversation turns (for multi-turn chat).
    pub fn build_chat_body<const N: usize>(
        &self,
        system: Option<&str>,
        turns: &[(Role, &str)],
        out: &mut String<N>,
    ) -> Result<(), ()> {
        out.clear();
        out.push_str("{\"model\":\"").map_err(|_| ())?;
        out.push_str(self.model).map_err(|_| ())?;
        // `stream_options.include_usage` asks the OpenAI-compatible endpoint to
        // emit a final usage chunk carrying `total_tokens`; without it the stream
        // never reports token counts.
        write!(
            out,
            "\",\"max_tokens\":{},\"stream\":true,\"stream_options\":{{\"include_usage\":true}},\"messages\":[",
            self.max_tokens
        )
        .map_err(|_| ())?;
        let mut first = true;
        if let Some(sys) = system {
            write_msg(out, "system", sys, &mut first)?;
        }
        for (role, text) in turns {
            write_msg(out, role.as_str(), text, &mut first)?;
        }
        out.push_str("]}").map_err(|_| ())?;
        Ok(())
    }
}

impl LlmProvider for OpenRouter<'_> {
    fn host(&self) -> &str {
        "openrouter.ai"
    }

    fn path(&self) -> &str {
        "/api/v1/chat/completions"
    }

    fn build_body<const N: usize>(&self, prompt: &str, out: &mut String<N>) -> Result<(), ()> {
        self.build_chat_body(None, &[(Role::User, prompt)], out)
    }

    fn extract_delta(&self, data_json: &str, out: &mut String<256>) -> bool {
        json::extract_openai_delta(data_json, out)
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

    #[test]
    fn builds_valid_openrouter_body() {
        let r = OpenRouter::new("deepseek/deepseek-chat");
        let mut body: String<512> = String::new();
        r.build_body("hi", &mut body).unwrap();
        assert_eq!(
            body.as_str(),
            r#"{"model":"deepseek/deepseek-chat","max_tokens":1024,"stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"user","content":"hi"}]}"#
        );
        assert_eq!(r.host(), "openrouter.ai");
        assert_eq!(r.path(), "/api/v1/chat/completions");
    }

    #[test]
    fn builds_multi_turn_body() {
        let r = OpenRouter::new("m");
        let mut b: String<512> = String::new();
        r.build_chat_body(
            Some("be brief"),
            &[
                (Role::User, "hi"),
                (Role::Assistant, "hello"),
                (Role::User, "bye"),
            ],
            &mut b,
        )
        .unwrap();
        assert_eq!(
            b.as_str(),
            r#"{"model":"m","max_tokens":1024,"stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"system","content":"be brief"},{"role":"user","content":"hi"},{"role":"assistant","content":"hello"},{"role":"user","content":"bye"}]}"#
        );
    }

    #[test]
    fn openrouter_extracts_delta() {
        let r = OpenRouter::new("deepseek/deepseek-chat");
        let mut out: String<256> = String::new();
        assert!(r.extract_delta(
            r#"{"choices":[{"delta":{"content":"yo"}}]}"#,
            &mut out
        ));
        assert_eq!(out.as_str(), "yo");
    }
}
