//! Pure, hardware-free core logic for the Sprig Pocket LLM Terminal.
//!
//! Everything here is `no_std` for the device build, but compiles with `std`
//! under `cargo test` so the logic can be verified on the host. There are no
//! hardware or async dependencies in this crate — only the keyboard state
//! machine, prediction interface, and JSON/SSE/provider parsing.
#![cfg_attr(not(test), no_std)]

pub mod button;
pub mod json;
pub mod keyboard;
pub mod layout;
pub mod predict;
pub mod provider;
pub mod sse;
