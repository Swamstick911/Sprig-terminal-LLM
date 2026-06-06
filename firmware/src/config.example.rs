//! Build-time configuration: WiFi credentials, LLM API key, model.
//!
//! Copy this file to `config.rs` (which is GITIGNORED) and fill in real values
//! before flashing. NEVER commit real secrets — `config.rs` is excluded from git
//! by the repo-root `.gitignore` (`/firmware/src/config.rs`).
//!
//! These are plain `&str` consts baked into the firmware image. They live in
//! flash, not in source control, once you fill in `config.rs`.
//!
//! The terminal talks to OpenRouter (an OpenAI-compatible gateway), so you can
//! use many models through one key — e.g. DeepSeek. Get a key at
//! https://openrouter.ai/keys and pick a model id from
//! https://openrouter.ai/models.

/// SSID of the 2.4 GHz WPA2 network the terminal joins.
pub const WIFI_SSID: &str = "your-ssid";

/// WPA2 passphrase for [`WIFI_SSID`].
pub const WIFI_PASSWORD: &str = "your-password";

/// OpenRouter API key, sent as `Authorization: Bearer <key>`. Format: `sk-or-...`.
pub const API_KEY: &str = "sk-or-REPLACE_ME";

/// OpenRouter model id used for Send and Expand, e.g. "deepseek/deepseek-chat".
pub const MODEL: &str = "deepseek/deepseek-chat";
