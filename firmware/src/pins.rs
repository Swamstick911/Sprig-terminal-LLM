//! RP2040 GPIO assignments for the Hack Club Sprig console.
//!
//! These are the *physical* pin numbers wired on the Sprig PCB. They were taken
//! from the official Sprig firmware HAL, which is the authoritative source:
//!
//!   - Buttons + LEDs: `firmware/sprig_hal/src/HAL.c`
//!     (`uint button_pins[] = {5, 7, 6, 8, 12, 14, 13, 15};`, indexed by the
//!     `Sprig_Button` enum order `W, S, A, D, I, K, J, L`).
//!     <https://github.com/hackclub/sprig/blob/main/firmware/sprig_hal/src/HAL.c>
//!   - Display SPI: `firmware/sprig_hal/src/ST7735_TFT.h`
//!     (`SPI_TFT_PORT spi0`, `SPI_SCK 18`, `SPI_TX 19`, `SPI_RX 16`,
//!      `SPI_TFT_CS 20`, `SPI_TFT_DC 22`, `SPI_TFT_RST 26`; backlight on 17).
//!     <https://github.com/hackclub/sprig/blob/main/firmware/sprig_hal/src/ST7735_TFT.h>
//!
//! Confidence:
//!   - Button pins: HIGH (verbatim from HAL.c, cross-checked against community
//!     ports and the Arduino-forum Sprig pin list).
//!   - SPI SCK/TX/CS/DC/RST + backlight: HIGH (verbatim from ST7735_TFT.h).
//!   - SPI peripheral = `SPI0`: HIGH. GPIO16/18/19 are valid SPI0 RX/SCK/TX on
//!     RP2040, consistent with the HAL.
//!
//! Anything marked `// TODO: verify` below could not be pinned down to a primary
//! source and uses a best-guess default.
//!
//! These constants are *documentation of the wiring*: embassy-rp identifies pins
//! by typed singletons (`p.PIN_18`, ...), not by `u8`, so the values here aren't
//! consumed directly — `main.rs` must keep its `p.PIN_xx` choices in sync with
//! them. Hence the module-wide `dead_code` allow.
#![allow(dead_code)]

/// Display (ST7735, 160x128, on `SPI0`) pin assignments.
pub mod display {
    /// SPI clock (SCK). HAL: `SPI_SCK 18`.
    pub const SCK: u8 = 18;
    /// SPI MOSI / TX (controller -> display). HAL: `SPI_TX 19`.
    pub const MOSI: u8 = 19;
    /// SPI MISO / RX. HAL: `SPI_RX 16`. The ST7735 is write-only in this design,
    /// so MISO is unused by the driver but the pin is reserved on the bus.
    pub const MISO: u8 = 16;
    /// Chip select (active low). HAL: `SPI_TFT_CS 20`.
    pub const CS: u8 = 20;
    /// Data/Command select (a.k.a. A0/RS). HAL: `SPI_TFT_DC 22`.
    pub const DC: u8 = 22;
    /// Reset (active low). HAL: `SPI_TFT_RST 26`.
    pub const RST: u8 = 26;
    /// Backlight / LED enable. HAL drives this high in `st7735_init()`.
    pub const BACKLIGHT: u8 = 17;
}

/// The eight button GPIOs.
///
/// On the Sprig the buttons connect GPIO -> GND and rely on the RP2040 internal
/// pull-ups, so a pressed button reads **low**. The input layer configures each
/// pin as input-with-pull-up and treats `is_low()` as "pressed".
pub mod buttons {
    /// W (left pad, up). HAL button_pins[Button_W] = 5.
    pub const W: u8 = 5;
    /// A (left pad, left). HAL button_pins[Button_A] = 6.
    pub const A: u8 = 6;
    /// S (left pad, down). HAL button_pins[Button_S] = 7.
    pub const S: u8 = 7;
    /// D (left pad, right). HAL button_pins[Button_D] = 8.
    pub const D: u8 = 8;
    /// I (right cluster, up). HAL button_pins[Button_I] = 12.
    pub const I: u8 = 12;
    /// J (right cluster, left). HAL button_pins[Button_J] = 13.
    pub const J: u8 = 13;
    /// K (right cluster, down). HAL button_pins[Button_K] = 14.
    pub const K: u8 = 14;
    /// L (right cluster, right). HAL button_pins[Button_L] = 15.
    pub const L: u8 = 15;
}

/// Audio (MAX98357A I2S class-D amplifier + speaker) pin assignments.
///
/// Source: the official Sprig firmware build config selects the Pico-SDK
/// `pico_audio_i2s` driver with these compile definitions in
/// `firmware/spade/src/rpi/CMakeLists.txt`:
///
/// ```text
///   PICO_AUDIO_I2S_DATA_PIN=9
///   PICO_AUDIO_I2S_CLOCK_PIN_BASE=10
///   PICO_AUDIO_I2S_MONO_INPUT=1
/// ```
///
/// <https://github.com/hackclub/sprig/blob/main/firmware/spade/src/rpi/CMakeLists.txt>
///
/// In the Pico-SDK `pico_audio_i2s` convention, `CLOCK_PIN_BASE` is BCLK and
/// `CLOCK_PIN_BASE + 1` is LRCLK (word-select) — the two must be consecutive
/// GPIOs because the PIO clocks them out on adjacent side-set pins. So:
///
///   - DIN  (serial data)  = GPIO 9   (`DATA_PIN`)
///   - BCLK (bit clock)    = GPIO 10  (`CLOCK_PIN_BASE`)
///   - LRCLK (word select) = GPIO 11  (`CLOCK_PIN_BASE + 1`)
///
/// Confidence:
///   - DATA=9, BCLK=10: HIGH (verbatim compile definitions from the Sprig
///     `rpi/CMakeLists.txt`).
///   - LRCLK=11: HIGH (it is `CLOCK_PIN_BASE + 1` by the fixed `pico_audio_i2s`
///     pin convention; the SDK driver derives word-select this way and the
///     hardware is wired to match).
///
/// The MAX98357A's `SD` (shutdown / gain-select) pin is left to the board's
/// default (amp enabled) and is not driven by firmware on the Sprig.
pub mod audio {
    /// I2S serial data (DIN on the MAX98357A). Sprig: `PICO_AUDIO_I2S_DATA_PIN=9`.
    pub const DIN: u8 = 9;
    /// I2S bit clock (BCLK). Sprig: `PICO_AUDIO_I2S_CLOCK_PIN_BASE=10`.
    pub const BCLK: u8 = 10;
    /// I2S word select / left-right clock (LRCLK). `CLOCK_PIN_BASE + 1 = 11`.
    pub const LRCLK: u8 = 11;
}

/// On-board status LEDs (not used in Milestone 1, kept for reference).
pub mod led {
    /// Left LED. HAL: `pin_num_led_l() == 28`.
    pub const LEFT: u8 = 28;
    /// Right LED. HAL: `pin_num_led_r() == 4`.
    pub const RIGHT: u8 = 4;
}
