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

## Setup (from scratch)

Your WiFi name, WiFi password and API key get built **into** the firmware, so
everyone builds their own copy. It sounds like a lot, but it's really just
"install three tools, paste four lines, run two commands." Takes about 15
minutes the first time.

### 1. Install the tools (one time only)

Works the same on Windows, macOS and Linux.

1. **Install Rust** from <https://rustup.rs> (follow the prompts; accept the
   defaults). When it finishes, close and reopen your terminal.
2. **Add the chip the Sprig uses:**
   ```sh
   rustup target add thumbv6m-none-eabi
   ```
3. **Install the flashing tool:**
   ```sh
   cargo install elf2uf2-rs
   ```

### 2. Get the code

```sh
git clone https://github.com/Swamstick911/Sprig-terminal-LLM.git
cd Sprig-terminal-LLM
```

### 3. Add your WiFi and API key

Make your own config file from the template (it's gitignored, so your secrets
are never uploaded):

```sh
cp firmware/src/config.example.rs firmware/src/config.rs
```

Open `firmware/src/config.rs` in any text editor and fill in the four values:

- `WIFI_SSID` — your WiFi network name (must be **2.4 GHz**, the Pico can't see
  5 GHz networks).
- `WIFI_PASSWORD` — that network's password.
- `API_KEY` — an OpenRouter key (starts with `sk-or-`). Make a free account at
  <https://openrouter.ai>, then create a key at <https://openrouter.ai/keys>.
  You'll need a little credit on the account for the model to reply.
- `MODEL` — which AI to use, e.g. `openai/gpt-5` or `deepseek/deepseek-chat`.
  Full list at <https://openrouter.ai/models>.

### 4. Build it

```sh
cd firmware
cargo build --release
```

The **first** build downloads and compiles a lot of code — give it a few
minutes. Later builds are fast.

### 5. Flash it onto the Sprig

1. **Hold down the BOOTSEL button** on the Pico, and *while still holding it*,
   plug the Sprig into your computer with USB. A new drive called **`RPI-RP2`**
   appears — now you can let go.
2. Turn the build into a `.uf2` file:
   ```sh
   elf2uf2-rs target/thumbv6m-none-eabi/release/sprig-llm-firmware sprig.uf2
   ```
3. **Copy `sprig.uf2` onto the `RPI-RP2` drive** (drag-and-drop works). The
   Sprig reboots on its own and starts the terminal. That's it.

To put new firmware on later, just repeat step 5.

### 6. Power it

Once flashed it runs on its own — no computer needed. Power it from a USB wall
charger or a power bank. (Plugging into a PC also works, and lets you use the
"type the reply to my PC" button.)

> **Heads up:** while a reply is generating the screen can flicker a little —
> that's a brief power dip when the WiFi radio transmits. A good power bank or
> wall charger keeps it steady; weak/old batteries make it worse. See
> [Troubleshooting](#troubleshooting).

---

Optional — run the logic tests on your computer (no hardware needed):

```sh
cargo test -p sprig-llm-core
```

Got a debug probe? Switch the runner in `firmware/.cargo/config.toml` to
`elf2uf2-rs -d` (or `probe-rs`) and just run `cargo run --release` instead of
steps 4–5.

## Troubleshooting

- **No `RPI-RP2` drive appears.** Hold **BOOTSEL** *before* you plug in and keep
  holding until the drive shows up. Try a different USB cable — some are
  charge-only and don't carry data.
- **The screen flickers while it's thinking.** Normal-ish: it's a power dip when
  the radio transmits. Use a quality power bank or wall charger (not tired AA
  batteries), or run it off your PC's USB for the steadiest power.
- **It connects but never replies / shows an error.** Check three things: your
  `API_KEY` is an OpenRouter `sk-or-` key, the OpenRouter account has some
  credit, and `MODEL` is a full slug like `openai/gpt-5` (not just `gpt-5`).
- **WiFi won't connect.** The Pico only sees **2.4 GHz** networks. Make sure
  `WIFI_SSID`/`WIFI_PASSWORD` are for a 2.4 GHz network with WPA2.
- **`cargo build` complains about the target.** Re-run
  `rustup target add thumbv6m-none-eabi`.

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
