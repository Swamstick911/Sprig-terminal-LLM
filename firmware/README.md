# Sprig Pocket LLM Terminal — Firmware (Milestone 1)

Embassy firmware for a Hack Club **Sprig** console (Raspberry Pi **Pico WH**:
RP2040 + CYW43439). Milestone 1 brings up the board, drives the 160x128 ST7735
LCD, reads the eight buttons, and runs the twin-pad keyboard from the
host-tested [`sprig-llm-core`](../crates/core) crate. **No WiFi/TLS yet** — that
is Milestone 2.

## What it does

1. `embassy_rp::init` brings up clocks/power and hands out the GPIO/peripheral
   singletons.
2. Configures `SPI0` and the ST7735, clears the screen.
3. Configures the 8 buttons as inputs with pull-ups (pressed = low).
4. Main loop, every ~5 ms:
   - scans + debounces the buttons → `KeyEvent`s (`Tap`, plus `Hold` once a
     button passes ~0.5 s — used by the core for `Hold(L)` = action layer),
   - feeds each event into `sprig_llm_core::keyboard::Keyboard` with a small
     `StaticPredictor` word list,
   - on `Outcome::Redraw` (or a `Send`/`Expand` placeholder banner) repaints the
     four screen zones with `embedded-graphics`.

`Send` and `Expand` only flash a status banner in Milestone 1.

### Screen zones (`src/display.rs`)

```
STATUS  ........................  CAPS ACT   <- label + caps/action flags
draft text (wraps, block cursor) ...........
pred: I:word  J:word  K:word  L:word .......  <- accept with the right pad
hint: abcd efgh ijkl ... / action labels ...  <- compose groups or actions
```

## Source layout

```
firmware/
  Cargo.toml          standalone crate (own [workspace]; thumbv6m-only deps)
  .cargo/config.toml  target = thumbv6m-none-eabi, runner, defmt linker args
  memory.x            RP2040 flash/RAM map (boot2 + 2 MiB flash + 264 KiB RAM)
  build.rs            puts memory.x on the linker search path
  src/
    main.rs           #[embassy_executor::main] entry + main loop
    pins.rs           verified Sprig GPIO assignments (the pinout)
    input.rs          8-button scan/debounce/hold -> KeyEvent
    display.rs        ST7735 init + four-zone embedded-graphics renderer
  README.md           this file
```

## Pin assignments

Taken from the **official Sprig firmware HAL** (authoritative):
`firmware/sprig_hal/src/ST7735_TFT.h` and `.../HAL.c` in
[`hackclub/sprig`](https://github.com/hackclub/sprig).

### Display — ST7735 on `SPI0`

| Signal     | GPIO | Source              | Confidence |
|------------|------|---------------------|------------|
| SCK        | 18   | `SPI_SCK 18`        | High       |
| MOSI / TX  | 19   | `SPI_TX 19`         | High       |
| MISO / RX  | 16   | `SPI_RX 16`         | High (unused by panel) |
| CS         | 20   | `SPI_TFT_CS 20`     | High       |
| DC (A0/RS) | 22   | `SPI_TFT_DC 22`     | High       |
| RST        | 26   | `SPI_TFT_RST 26`    | High       |
| Backlight  | 17   | driven high in `st7735_init()` | High |

> One community/forum port listed CS=21 / DC=22 / RST=26; the HAL header
> (CS=20, DC=22, RST=26) is the primary source and is what this firmware uses.

### Buttons (input pull-up, pressed = low)

`HAL.c`: `uint button_pins[] = {5, 7, 6, 8, 12, 14, 13, 15};` indexed by the
`Sprig_Button` enum order `W, S, A, D, I, K, J, L`, giving:

| Button | GPIO | | Button | GPIO |
|--------|------|-|--------|------|
| W      | 5    | | I      | 12   |
| A      | 6    | | J      | 13   |
| S      | 7    | | K      | 14   |
| D      | 8    | | L      | 15   |

Confidence: **High** for all eight (verbatim from `HAL.c`, cross-checked against
the OSHWLab schematic and community ports).

On-board LEDs (left=GPIO28, right=GPIO4) are recorded in `pins::led` for later
use but are not driven in Milestone 1.

**No `// TODO: verify` pins remain** — every pin above traces to the HAL source.

## Building

Requires the `thumbv6m-none-eabi` target:

```sh
rustup target add thumbv6m-none-eabi
cd firmware
cargo build --release
```

This builds and links cleanly to an ELF at
`target/thumbv6m-none-eabi/release/sprig-llm-firmware`.

## Flashing

### Option A — probe-rs (SWD debug probe; gives RTT logs)

Best dev loop. Use a Picoprobe or a second Pico flashed as a debug probe wired
to SWDIO/SWCLK/GND.

```sh
cargo install probe-rs-tools
cargo run --release          # runner = `probe-rs run --chip RP2040`
```

`defmt` logs stream back over RTT. (`.cargo/config.toml` sets this runner.)

### Option B — UF2 bootloader (no probe, USB mass storage)

```sh
cargo install elf2uf2-rs
```

Edit `.cargo/config.toml` to select the elf2uf2 runner, then hold **BOOTSEL**
while plugging the Pico in (it mounts as `RPI-RP2`) and:

```sh
cargo run --release          # flashes the UF2 to the mounted drive
```

Or convert manually and drag-drop:

```sh
elf2uf2-rs target/thumbv6m-none-eabi/release/sprig-llm-firmware firmware.uf2
# copy firmware.uf2 onto the RPI-RP2 drive
```

## Workspace integration

The crate carries an empty `[workspace]` table so it builds **standalone** and
stays out of the repo-root workspace's host-only `cargo test`. To fold it into
the root workspace later:

1. Delete the `[workspace]` table from `firmware/Cargo.toml`.
2. Add `"firmware"` to `members` in the root `Cargo.toml`.
3. Keep it out of the default host build (it only targets thumbv6m): either
   list it under `default-members` *excluding* firmware, or rely on
   `.cargo/config.toml`'s `target = thumbv6m-none-eabi` (note: a workspace-level
   default target affects all members, so per-crate isolation via
   `default-members` is usually cleaner).
4. Mixed-target workspaces share one `Cargo.lock`; verify the host crates still
   resolve after adding the embedded deps.

The path dependency `sprig-llm-core = { path = "../crates/core" }` already
points at the existing core crate and compiles in `no_std` mode here.

## Notable dependency notes

- **`embassy-rp 0.2`** is RP2040-only and has *no* `rp2040` chip feature (that
  arrived in 0.3+ with RP2350). Features used: `rt`, `time-driver`,
  `critical-section-impl`, `boot2-w25q080` (the Pico's Winbond-class QSPI flash),
  `defmt`.
- **`st7735-lcd 0.10`** expects an embedded-hal `SpiDevice`, so the raw blocking
  `Spi` bus + CS pin are wrapped in `embedded-hal-bus`'s `ExclusiveDevice`.
- **`portable-atomic`** (pulled by `embedded-hal-bus`) is configured with the
  `critical-section` feature because Cortex-M0+ has no native atomic CAS.

## Milestone 2 (next)

- CYW43439 WiFi bring-up via `cyw43` + `embassy-net` (the Pico W radio is on an
  SPI link through the RP2040's PIO; needs the `cyw43` firmware blobs).
- TLS (`embassy-net` + `embedded-tls`) to an LLM endpoint.
- Wire `Outcome::Send` / `Outcome::Expand` to an HTTPS request and stream the
  reply (the core already has SSE/provider/JSON parsing in
  `sprig_llm_core::{sse, provider, json}`).
- Replace the `StaticPredictor` seed list with the flash-resident trie/bigram
  predictor.
