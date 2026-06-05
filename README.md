# Sprig Pocket LLM Terminal

Software-only firmware for a Hack Club **Sprig** (Raspberry Pi Pico WH) that turns
the console into a self-contained AI chat terminal: type a prompt on the 8-button
keypad, send it over WiFi to a hosted LLM, and stream the reply to the 160×128 LCD.

The signature feature is **fast text entry on 8 buttons**: a deterministic
"twin-pad two-stage" keyboard + on-device next-word prediction + an
"expand-with-AI" action that turns shorthand into a full prompt.

See [`docs/specs/2026-06-06-pocket-llm-terminal-design.md`](docs/specs/2026-06-06-pocket-llm-terminal-design.md)
for the full design.

## Layout

```
crates/core/   # no_std, host-testable pure logic (keyboard, prediction, SSE/provider parsing)
firmware/      # Embassy firmware for the Pico WH (added in Milestone 1)
tools/         # offline dictionary/n-gram builder (host)
```

## Core logic

```sh
cargo test          # run the core logic test suite (host)
```

Built in Rust. Stack: Embassy + cyw43 + embassy-net + reqwless/embedded-tls +
serde-json-core + mipidsi/embedded-graphics.
