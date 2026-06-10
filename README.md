# Sprig Pocket LLM Terminal

A Hack Club **Sprig** (Raspberry Pi Pico WH) turned into a self-contained AI chat
terminal — **software only, no extra hardware**. Type a prompt on the 8 buttons,
it goes over WiFi to an LLM (via OpenRouter), and the reply streams onto the
160×128 screen. It also plays AI games, can type answers into your PC over USB,
and remembers your chats across power-offs.

## Features

- **Twin-pad keyboard** — fast, deterministic text entry on 8 buttons (two
  D-pads): tap the on-screen letter group, then the letter. Nothing to memorise.
- **Prediction** — a ~14.6k-word frequency dictionary with inline "ghost text"
  completion; press Space to accept the suggestion.
- **Multi-turn chat** — keeps conversation context across messages.
- **Any model via OpenRouter** — switch models on-device (e.g. `openai/gpt-5`,
  `deepseek/deepseek-chat`).
- **Settings menu** — model, persona (Concise / Coder / Translate), max-tokens,
  quick prompts, AI game modes, and "new conversation".
- **AI game modes** — text adventure, 20 Questions, and trivia, with the LLM as
  game master.
- **Response scrolling** and a live **token-usage readout** (per reply + session).
- **USB type-to-PC** — the device is also a USB keyboard; it can type the reply
  straight into the focused app on your computer.
- **Audio** — soft key clicks and a reply-ready chime (PIO-driven I²S to the
  MAX98357A).
- **Persistence** — settings and conversation are saved to flash and restored on
  boot.

## Hardware

Sprig with a **Raspberry Pi Pico WH** (RP2040 + CYW43439 WiFi, 264 KB RAM, 2 MB
flash), 160×128 ST7735 LCD, 8 buttons, and a MAX98357A I²S speaker. Needs a
2.4 GHz WPA2 network.

## Repo layout

```
crates/core/         no_std, host-tested logic — keyboard state machine,
                     prediction, and JSON/SSE/provider parsing
firmware/            Embassy firmware for the Pico WH
tools/dict-builder/  host tool: turns a word-frequency corpus into the
                     include!-able on-device dictionary
docs/specs/          design document
```

## Configure, build & flash

Prerequisites: Rust with the `thumbv6m-none-eabi` target, and `elf2uf2-rs`
(`cargo install elf2uf2-rs`) for UF2 flashing — or `probe-rs` if you have a debug
probe.

1. **Configure** (one-time). Copy the template to `config.rs` (which is
   gitignored — your secrets never get committed) and fill it in:
   ```sh
   cp firmware/src/config.example.rs firmware/src/config.rs
   ```
   Set `WIFI_SSID`, `WIFI_PASSWORD`, an OpenRouter `API_KEY` (`sk-or-…`, from
   <https://openrouter.ai/keys>), and `MODEL` (e.g. `openai/gpt-5` or
   `deepseek/deepseek-chat`).

2. **Build:**
   ```sh
   cd firmware
   cargo build --release
   ```

3. **Flash** (UF2): hold **BOOTSEL** while plugging in the Sprig so it mounts as
   the `RPI-RP2` drive, then:
   ```sh
   elf2uf2-rs target/thumbv6m-none-eabi/release/sprig-llm-firmware sprig.uf2
   # then copy sprig.uf2 onto the RPI-RP2 drive (drag-and-drop or cp)
   ```
   With a debug probe instead, switch the runner in `firmware/.cargo/config.toml`
   to `elf2uf2-rs -d` (or `probe-rs`) and run `cargo run --release`.

Run the host-side logic tests:
```sh
cargo test -p sprig-llm-core
```

## Controls

The 8 buttons are a left D-pad (`W` `A` `S` `D`) and a right cluster
(`I` `J` `K` `L`).

- **Type a letter** (two taps): tap the group button shown on screen, then the
  left-pad button for the letter in that group.
- **Space / accept suggestion:** `L` — accepts the grey ghost completion if one
  is shown, otherwise inserts a space.
- **Action layer:** hold `L`, then `A` send · `S` expand-with-AI · `W` backspace
  · `D` caps · `J` newline · `K` clear · `I` settings · `L` cancel.
- **Settings menu:** `W`/`I` up, `S`/`K` down, `A`/`L`/`D` change or select.
- **Reply view:** `W`/`I` scroll up, `S`/`K` scroll down, `D` types the reply to
  your PC over USB, any other key returns to the keyboard.

## Tech stack

Rust, `no_std`. Embassy (embassy-rp / embassy-net / embassy-time / embassy-usb),
`cyw43` for WiFi, `reqwless` + `embedded-tls` (TLS 1.3) for streaming HTTPS,
`st7735-lcd` + `embedded-graphics` for the display, PIO-driven I²S for audio, and
`embassy-rp` flash for persistence. The JSON and SSE parsing is a small,
allocation-free, hand-rolled implementation in `crates/core` (host-tested).

## Limitations

- TLS is **encrypted but the server certificate is not verified** — `reqwless`
  0.12 exposes no verification hook, so the connection is private but not
  authenticated. That's fine on a trusted network; full certificate verification
  needs a future upgrade of the embedded TLS stack. The API key is also baked
  into the firmware image.
- Tuned for short prompts and replies (bounded RAM); a very long conversation
  rolls the oldest turns off the history.

## License

MIT.
