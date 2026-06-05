# Pocket LLM Terminal — Design Spec

**Date:** 2026-06-06
**Target hardware:** Hack Club Sprig with **Raspberry Pi Pico WH** (RP2040 + CYW43439)
**Constraint:** software-only — device + USB cable, no soldering or add-ons
**Language:** Rust (`no_std`, Embassy async)

---

## 1. Goal

A self-contained handheld that connects to WiFi, lets the user compose a prompt on the 8-button keypad, sends it to a hosted LLM API over HTTPS, and streams the reply to the 160×128 LCD. The headline problem this design solves is **fast text entry on 8 buttons**, addressed with a deterministic two-tap keyboard plus on-device prediction and an LLM-powered "expand shorthand" action.

### Success criteria
- Boots, joins a configured WiFi network, syncs time via NTP.
- User can compose a multi-word prompt and send it.
- The model's reply streams onto the LCD token-by-token (no full-response buffering).
- Typing a short word takes noticeably fewer than 4 presses/letter on average thanks to prediction.
- "Expand with AI" turns terse shorthand into a full prompt.

### Non-goals (YAGNI)
- BLE, multiplayer, voice. No conversation history persistence beyond the current session. No on-screen settings editor in v1 (WiFi creds + API key are flashed in via a config, see §8).

---

## 2. Hardware constraints

| Resource | Value | Implication |
|---|---|---|
| MCU | RP2040 dual-core @ up to 133 MHz | core0 = app/UI, core1 available for prediction/render |
| RAM | 264 KB SRAM | TLS buffers are the tight resource; stream, never buffer |
| Flash | 2 MB QSPI | dictionary + n-gram model live here, memory-mapped (XIP) |
| Wireless | CYW43439 — 2.4 GHz WiFi + BLE | WiFi only for v1 |
| Display | ST7735 160×128 color, SPI | text renderer over `embedded-graphics` |
| Input | 8 buttons: left pad `W/A/S/D`, right pad `I/J/K/L` | the keyboard (see §5) |
| Audio | MAX98357A I²S | optional UI click/notify sounds (v2) |

---

## 3. Architecture

Modules, each with one clear responsibility and a narrow interface:

```
┌────────────────────────────────────────────────────────────┐
│ app  (core0 main loop: state machine, orchestration)        │
└───┬───────────┬────────────┬───────────┬────────────────────┘
    │           │            │           │
┌───▼───┐  ┌────▼─────┐  ┌───▼────┐  ┌───▼────────┐
│ input │  │ keyboard │  │ display│  │ llm        │
│ (GPIO │  │ (twin-   │  │ (ST7735│  │ (provider  │
│  scan,│  │  pad +   │  │  +     │  │  trait +   │
│ debounce)│ predict) │  │ render)│  │  TLS/SSE)  │
└───────┘  └────┬─────┘  └────────┘  └───┬────────┘
                │                        │
          ┌─────▼──────┐           ┌─────▼──────┐
          │ predict    │           │ net        │
          │ (trie +    │           │ (cyw43 +   │
          │  n-gram,   │           │  embassy-  │
          │  flash XIP)│           │  net+NTP)  │
          └────────────┘           └────────────┘
```

- **input** — debounced edge events per button: `Pressed(Btn)`, `Released(Btn)`, `Held(Btn)`.
- **keyboard** — consumes input events + prediction candidates, owns the keyboard state machine (§5), emits `TextEvent`: `Char(c)`, `Word(&str)`, `Space`, `Backspace`, `Send`, `Expand`, `ModeChange`.
- **predict** — given the current text buffer, returns ≤4 candidate words. Pure lookups against flash-resident trie + bigram model. No allocation.
- **display** — renders the three screen zones (§6) from app state. Double-buffered DMA SPI; dirty-region updates.
- **llm** — `LlmProvider` trait + per-provider module. Builds the request, opens TLS, writes the request, parses the streamed SSE body, yields text deltas.
- **net** — WiFi bring-up (`cyw43`), DHCP/DNS (`embassy-net`), NTP time sync, and a reusable TLS-capable TCP connection for `llm`.

---

## 4. Tech stack (Rust crates)

| Concern | Crate |
|---|---|
| Async runtime + HAL | `embassy-rp`, `embassy-executor`, `embassy-time` |
| WiFi | `cyw43`, `cyw43-pio` |
| TCP/IP | `embassy-net` (smoltcp) |
| HTTPS client | `reqwless` with `embedded-tls` feature (TLS 1.3) |
| JSON (SSE deltas) | `serde-json-core` (no_std) |
| Display | `mipidsi` or `st7735-lcd` + `embedded-graphics` |
| Logging/flash/debug | `defmt`, `defmt-rtt`, `probe-rs` for flashing |

**TLS note:** `embedded-tls` is **TLS 1.3 only**. Both Anthropic and OpenAI endpoints support TLS 1.3. Pin a single root CA and one ECDHE-AES-GCM ciphersuite to keep RAM down. De-risk this first (see §10, Milestone 2).

---

## 5. Input system — Twin-pad + Predict + Expand

### 5.1 Letter layout (7 groups + action key)

26 letters + 2 punctuation across **7 group buttons**; `L` is reserved as the action/space key.

```
 W: a b c d      I: q r s t
 A: e f g h      J: u v w x
 S: i j k l      K: y z . ,
 D: m n o p      L: ── SPACE / ACTION ──
```

### 5.2 The two taps (deterministic)

- **COMPOSE state (default):** the 7 group buttons are live. Tap one → enter LETTER state for that group. `L` tap = Space. `L` hold = Action layer (§5.4).
- **LETTER state:** the chosen group's ≤4 letters map to the **left pad** `W=1st A=2nd S=3rd D=4th`. Tap one → that letter is committed, return to COMPOSE. The **right pad** `I/J/K/L` = accept predicted word #1–4 (see 5.3). Any group re-tap is ignored until a selection or timeout.

Worked example — "hi": tap `S` (ijkl) → screen shows `i=W j=A k=S l=D` → tap `W` = **i**? No — h is in group `A` (efgh): tap `A` → `e=W f=A g=S h=D` → tap `D` = **h**; tap `S` (ijkl) → tap `W` = **i**. 4 presses, fully guided.

### 5.3 Prediction (on-device)

After each committed letter, `predict` returns ≤4 candidate words (trie completion ranked by a bigram next-word model). They render in the prediction zone. In LETTER state the right pad accepts them: `I`=word1 … `L`=word4. Accepting a word inserts it + a trailing space and returns to COMPOSE. This is the everyday speed multiplier — common words become 1–2 presses.

### 5.4 Action layer (hold `L`)

While `L` is held, the 7 group buttons become actions; release fires the highlighted one (or tap-through):
`W`=Backspace · `A`=Send · `S`=**Expand-with-AI** · `D`=Caps toggle · `I`=Symbols/numbers layer · `J`=Newline · `K`=Cancel/clear.

### 5.5 Expand-with-AI

`Expand` sends the current draft to the same provider with a fixed system instruction ("Rewrite this terse note into a clear, complete prompt; output only the rewritten prompt."). The returned text **replaces** the draft for review; the user then hits Send. Never in the per-keystroke loop — only on explicit action, so typing latency is unaffected.

---

## 6. Screen layout (160×128)

```
┌──────────────────────────────┐
│ status: wifi ▮ time  mode:CMP │  row 0   (8px)
├──────────────────────────────┤
│ draft / response text         │  body    (scrolls)
│ ...                           │
├──────────────────────────────┤
│ [hello] [help] [held] [hex]   │  predict (16px) — or active group's letters
├──────────────────────────────┤
│ grp: W:abcd A:efgh … L:SPACE  │  hint   (8px)
└──────────────────────────────┘
```

Two phases: **composing** (body shows draft + cursor; predict row shows candidates) and **receiving** (body streams the reply; predict row shows a spinner + token count).

---

## 7. Network → LLM pipeline

### 7.1 Provider abstraction

```rust
trait LlmProvider {
    // Build HTTP request line + headers + JSON body for a streaming chat call.
    fn request(&self, prompt: &str, buf: &mut [u8]) -> Request<'_>;
    // Given one decoded SSE `data:` payload, return Some(text_delta) or None.
    fn parse_delta<'a>(&self, sse_json: &'a str) -> Option<&'a str>;
}
```

Two impls: `Claude` (default) and `OpenAi`. Switching providers = constructing a different `LlmProvider`. (LLM provider choice is the one open decision — see §11.)

### 7.2 Claude (default) wire details

- **Endpoint:** `POST https://api.anthropic.com/v1/messages`
- **Headers:** `x-api-key: <key>`, `anthropic-version: 2023-06-01`, `content-type: application/json`
- **Body:**
  ```json
  {"model":"claude-opus-4-8","max_tokens":1024,"stream":true,
   "messages":[{"role":"user","content":"<prompt>"}]}
  ```
- **Model:** default `claude-opus-4-8`. For a snappier/cheaper pocket device the user may instead choose `claude-haiku-4-5` — this is the user's call (see §11), not a silent downgrade.
- **Thinking:** omitted (adaptive thinking is off when the field is absent) → lowest latency, simplest output for a tiny screen.
- **Streaming SSE events to handle:**
  - `content_block_delta` → `delta.type == "text_delta"` → append `delta.text` to the LCD.
  - `message_delta` (carries `stop_reason`, `usage.output_tokens`) → update token counter.
  - `message_stop` → done.
  - `error` event → surface message, abort.

### 7.3 Streaming parse (RAM-safe)

`reqwless` exposes the response body as an incremental reader. The pipeline: read TLS plaintext chunk → de-chunk HTTP/1.1 → accumulate one SSE `data:` line at a time → `serde-json-core` scan for the delta field → push text straight to the renderer. Peak extra RAM is one SSE line (a few hundred bytes), never the whole response. This is why streaming is *easier* here, not harder.

### 7.4 Failure handling
- WiFi join fail / DHCP timeout → status-bar error, retry with backoff.
- TLS handshake fail → show short code (most likely cause: RAM/buffer sizing), retry once.
- HTTP 401 → "check API key"; 429 → "rate limited, wait"; 5xx → retry with backoff.
- Stream drop mid-reply → keep partial text, mark truncated.

---

## 8. Flash & config layout (2 MB)

| Region | Contents |
|---|---|
| Firmware | the Rust binary |
| Dictionary | packed trie, ~100–500 KB (word-frequency list, built offline) |
| N-gram | pruned bigram next-word model, few hundred KB |
| Config | WiFi SSID/password, provider, API key, model id — a small flashed blob (`config.rs` or a reserved flash sector) |
| Root CA | one pinned cert for the chosen API host |

Dictionary/n-gram are generated by an **offline build tool** (host-side Rust/Python) from a word-frequency corpus and embedded via `include_bytes!` or written to a reserved flash sector; lookups are memory-mapped (XIP), so they cost ~0 RAM.

---

## 9. Build & flash workflow

- `cargo build --release` (thumbv6m target), flash via `probe-rs` / `cargo run` with a debug probe, or UF2 over USB bootloader.
- `defmt` logs over RTT for debugging.
- Offline dictionary builder is a separate host binary in `tools/`.

---

## 10. Milestones (for ralph-loop execution)

1. **Blink + LCD + buttons** — Embassy project boots; draw text on the ST7735; print button events. *Proves the toolchain + display + input.*
2. **WiFi + HTTPS spike** — join WiFi, NTP sync, do ONE streaming `POST /v1/messages` against the chosen host and print deltas over RTT. *De-risks `embedded-tls` heap tuning — the single biggest risk; do this early.*
3. **Twin-pad keyboard** — the §5 state machine end to end: compose a string with the two-tap method, Space/Backspace/Send via the action layer. No prediction yet.
4. **On-device prediction** — offline dictionary/n-gram builder + flash embedding + the predict module + right-pad word accept.
5. **Full loop on-device** — compose on keypad → stream reply to LCD, with status/predict/hint zones.
6. **Expand-with-AI** — the shorthand→prompt action.
7. **Polish** — caps/symbols layer, scrolling, error states, optional I²S click sounds.

---

## 11. Open decisions

1. **LLM provider** — designed provider-agnostic; **Claude `claude-opus-4-8` is the default first target**. The user may switch to OpenAI (one module) or to `claude-haiku-4-5` for lower latency/cost. To finalize, the user picks a provider + model and supplies an API key. Non-blocking for milestones 1–3.
2. **Action-layer ergonomics** — the hold-`L` action layer (§5.4) is the proposed scheme; validate feel in Milestone 3 and adjust mappings if needed.
