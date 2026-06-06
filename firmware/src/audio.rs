//! Audio feedback for the Sprig: short UI sounds on the MAX98357A I2S speaker.
//!
//! The Sprig drives a MAX98357A class-D amplifier over I2S. The RP2040 has **no
//! hardware I2S peripheral**, so — exactly like the official Sprig firmware,
//! which uses the Pico-SDK `pico_audio_i2s` driver — we synthesise the I2S TX
//! frame in a small PIO program that clocks out BCLK, LRCLK (word-select) and
//! the serial data line. See [`crate::pins::audio`] for the (verified) pinout:
//!
//!   - DIN  (serial data)  = GPIO 9
//!   - BCLK (bit clock)    = GPIO 10
//!   - LRCLK (word select) = GPIO 11
//!
//! ## PIO / DMA choice
//! `net.rs` already uses **PIO0** (for the CYW43 WiFi PIO-SPI) and `DMA_CH0`.
//! To avoid any conflict, audio uses **PIO1** and a separate DMA channel
//! (`main.rs` passes `p.PIO1` and e.g. `p.DMA_CH1`). Only state-machine 0 of
//! PIO1 is used.
//!
//! ## Frame format
//! MAX98357A expects standard I2S stereo frames: 32 bit-clocks per frame, MSB
//! first, with LRCLK selecting the channel. We send 16-bit samples per channel.
//! The Sprig amp sums L+R internally (its `PICO_AUDIO_I2S_MONO_INPUT` build),
//! but on the wire we still emit full stereo frames — we just duplicate the mono
//! sample into both the left and right 16-bit slots of each 32-bit FIFO word.
//!
//! ## Allocation-free
//! `no_std`, no heap. Samples are generated on the fly into a small fixed
//! `[u32; CHUNK]` stack buffer and streamed to the PIO TX FIFO by DMA, one chunk
//! at a time, so total memory is a few hundred bytes regardless of tone length.
//!
//! ## Volume
//! Output amplitude is deliberately modest (see [`AMPLITUDE`]) — these are quiet
//! UI ticks, not music.
//!
//! The whole module carries `#![allow(dead_code)]` because `main.rs` wires the
//! call sites (`Audio::init` / `play_click` / `play_chime`) separately; without
//! a caller yet the public API would otherwise warn as unused.
#![allow(dead_code)]

use embassy_rp::dma::Channel as DmaChannel;
use embassy_rp::peripherals::PIO1;
use embassy_rp::pio::{
    Common, Config, Direction, FifoJoin, Pio, PioPin, ShiftConfig, ShiftDirection, StateMachine,
};
use embassy_rp::Peripheral;
use embassy_rp::PeripheralRef;
use fixed::traits::ToFixed;

/// I2S sample rate (Hz). 22.05 kHz is plenty for short UI blips and keeps the
/// PIO clock-divider comfortably in range; samples are cheap to synthesise.
const SAMPLE_RATE: u32 = 22_050;

/// PIO cycles per I2S stereo frame.
///
/// The PIO program below spends 2 cycles per bit (one `out` + one `jmp`/`set`,
/// each toggling BCLK via side-set) and a frame is 32 bit-clocks (16 bits ×
/// 2 channels), so `32 × 2 = 64` PIO cycles per frame. The state-machine clock
/// must therefore run at `SAMPLE_RATE × 64`.
const CYCLES_PER_FRAME: u32 = 64;

/// Peak 16-bit sample magnitude. ~18% of full scale: audible but gentle through
/// the tiny Sprig speaker, and well clear of clipping.
const AMPLITUDE: i16 = 6000;

/// DMA chunk size (stereo frames). The TX FIFO is small (4 words), so we stream
/// in `CHUNK`-frame bursts; one DMA transfer per chunk. 256 frames ≈ 11.6 ms of
/// audio at 22.05 kHz, a good balance of buffer RAM (1 KiB) vs. DMA setup churn.
const CHUNK: usize = 256;

/// Click tone frequency (Hz) — a soft mid tick.
const CLICK_HZ: u32 = 1_000;
/// Click duration (ms). A brief soft tick, within the 10–20 ms spec.
const CLICK_MS: u32 = 14;

/// "Reply ready" chime: a short rising 3-note arpeggio (A5, C#6, E6 — an A-major
/// triad) at ~70 ms per note. Pleasant and clearly distinct from the click.
const CHIME_NOTES_HZ: [u32; 3] = [880, 1109, 1319];
/// Per-note duration (ms) for the chime.
const CHIME_NOTE_MS: u32 = 70;

/// The audio output: a configured PIO1 state machine plus its DMA channel,
/// ready to stream I2S frames to the MAX98357A.
pub struct Audio {
    sm: StateMachine<'static, PIO1, 0>,
    dma: PeripheralRef<'static, embassy_rp::dma::AnyChannel>,
}

impl Audio {
    /// Bring up the PIO-I2S audio output.
    ///
    /// Pass in PIO1, the three audio GPIOs (`data`/`bclk`/`lrclk` — on the Sprig
    /// these are `p.PIN_9`, `p.PIN_10`, `p.PIN_11`; `bclk`/`lrclk` MUST be
    /// consecutive pins with `bclk` lower, as the PIO side-sets them as a pair),
    /// and a free DMA channel (e.g. `p.DMA_CH1` — NOT `DMA_CH0`, which `net.rs`
    /// uses). PIO0/`DMA_CH0` are reserved for WiFi, so this never conflicts.
    ///
    /// The amplifier idles silent until [`Audio::play_click`] /
    /// [`Audio::play_chime`] are called; the state machine is left enabled and
    /// clocks out zero frames (DC silence) between sounds.
    pub fn init(
        pio: PIO1,
        irq: impl embassy_rp::interrupt::typelevel::Binding<
            embassy_rp::interrupt::typelevel::PIO1_IRQ_0,
            embassy_rp::pio::InterruptHandler<PIO1>,
        >,
        data: impl PioPin,
        bclk: impl PioPin,
        lrclk: impl PioPin,
        dma: impl Peripheral<P = impl DmaChannel> + 'static,
    ) -> Self {
        let Pio {
            mut common, sm0, ..
        } = Pio::new(pio, irq);

        let sm = configure_sm(&mut common, sm0, data, bclk, lrclk);

        Self {
            sm,
            dma: dma.into_ref().map_into(),
        }
    }

    /// Play a brief soft "key click" (~14 ms, 1 kHz). Awaits until the sound has
    /// been clocked out, then leaves the line silent.
    pub async fn play_click(&mut self) {
        let frames = ms_to_frames(CLICK_MS);
        self.play_tone(CLICK_HZ, frames).await;
        self.silence().await;
    }

    /// Play the short "reply ready" chime (a rising A-major arpeggio). Awaits
    /// until the whole chime has played, then leaves the line silent.
    pub async fn play_chime(&mut self) {
        let frames = ms_to_frames(CHIME_NOTE_MS);
        for &hz in CHIME_NOTES_HZ.iter() {
            self.play_tone(hz, frames).await;
        }
        self.silence().await;
    }

    /// Synthesise and stream `frames` stereo frames of a square wave at `hz`,
    /// with a short linear fade-in/out to suppress click artefacts at the edges.
    async fn play_tone(&mut self, hz: u32, frames: u32) {
        // Half-period in frames: the square wave flips sign every half period.
        // Guard against div-by-zero / absurdly high tones.
        let half_period = (SAMPLE_RATE / hz.max(1) / 2).max(1);
        // Fade ramp length (frames) — ~1.5 ms, clamped to at most a third of the
        // tone so very short tones still fade.
        let ramp = (SAMPLE_RATE / 666).min(frames / 3).max(1);

        let mut emitted: u32 = 0;
        let mut buf = [0u32; CHUNK];
        while emitted < frames {
            let n = core::cmp::min(CHUNK as u32, frames - emitted) as usize;
            for (i, slot) in buf.iter_mut().enumerate().take(n) {
                let idx = emitted + i as u32;
                // Square wave: +A for the first half period, -A for the second.
                let base = if (idx / half_period) & 1 == 0 {
                    AMPLITUDE
                } else {
                    -AMPLITUDE
                };
                // Linear fade in over the first `ramp` frames and out over the
                // last `ramp` frames, so the tone starts/ends without a pop.
                let gain_num = if idx < ramp {
                    idx + 1
                } else if idx >= frames - ramp {
                    frames - idx
                } else {
                    ramp
                };
                let sample = ((base as i32) * (gain_num as i32) / (ramp as i32)) as i16;
                *slot = stereo_word(sample);
            }
            self.sm
                .tx()
                .dma_push(self.dma.reborrow(), &buf[..n])
                .await;
            emitted += n as u32;
        }
    }

    /// Push a short run of zero (silent) frames so the amp settles at mid-rail
    /// and the DC level is defined between sounds.
    async fn silence(&mut self) {
        let buf = [0u32; 32];
        self.sm.tx().dma_push(self.dma.reborrow(), &buf).await;
    }
}

/// Pack a mono 16-bit `sample` into a single 32-bit I2S FIFO word carrying both
/// stereo channels (left in the high half, right in the low half). The PIO
/// program shifts MSB-first (shift-left), so the left channel's MSB goes out
/// first, which is the standard I2S framing the MAX98357A expects.
#[inline]
fn stereo_word(sample: i16) -> u32 {
    let s = (sample as u16) as u32;
    (s << 16) | s
}

/// Convert milliseconds to whole stereo frames at [`SAMPLE_RATE`].
#[inline]
fn ms_to_frames(ms: u32) -> u32 {
    (SAMPLE_RATE * ms / 1000).max(1)
}

/// Assemble the I2S TX PIO program, load it into PIO1, bind the pins, and return
/// the configured (enabled) state machine.
fn configure_sm(
    common: &mut Common<'static, PIO1>,
    mut sm: StateMachine<'static, PIO1, 0>,
    data: impl PioPin,
    bclk: impl PioPin,
    lrclk: impl PioPin,
) -> StateMachine<'static, PIO1, 0> {
    // Standard I2S philips framing, 16 bits/channel. Side-set drives 2 pins:
    //   side-set bit 0 -> BCLK   (the side-set *base* pin)
    //   side-set bit 1 -> LRCLK  (base + 1)
    // OUT shifts the serial data onto the DATA pin, MSB first (shift left), with
    // autopull at 32 bits (a full L+R FIFO word per frame).
    //
    // This mirrors the Pico-SDK `audio_i2s.pio` program: 2 PIO cycles per bit
    // (one to drive BCLK low while presenting the data bit, one to clock it high
    // / advance the loop), 16 bits per channel, LRCLK toggled on the channel
    // boundary so it changes one BCLK *before* the first bit of each channel.
    let prg = pio_proc::pio_asm!(
        ".side_set 2",
        // X = remaining-bits counter. Each channel sends 16 bits: the first bit
        // is emitted by the `out` just after the `set x, 14`, then the loop
        // emits 15 more (x = 14 -> jmps until x underflows), then a final `out`.
        "bitloop1:",
        "    out pins, 1   side 0b10", // BCLK lo, LRCLK hi (right channel)
        "    jmp x-- bitloop1 side 0b11", // BCLK hi, LRCLK hi
        "    out pins, 1   side 0b00", // last right bit; BCLK lo, LRCLK -> lo
        "    set x, 14     side 0b01", // BCLK hi, LRCLK lo; prime left channel
        "bitloop0:",
        "    out pins, 1   side 0b00", // BCLK lo, LRCLK lo (left channel)
        "    jmp x-- bitloop0 side 0b01", // BCLK hi, LRCLK lo
        "    out pins, 1   side 0b10", // last left bit; BCLK lo, LRCLK -> hi
        "public entry_point:",
        "    set x, 14     side 0b11", // BCLK hi, LRCLK hi; prime right channel
    );

    let data_pin = common.make_pio_pin(data);
    let bclk_pin = common.make_pio_pin(bclk);
    let lrclk_pin = common.make_pio_pin(lrclk);

    let mut cfg = Config::default();
    // Side-set base = BCLK; the second side-set pin (LRCLK) must be BCLK+1, which
    // is how the Sprig is wired (BCLK=10, LRCLK=11).
    cfg.use_program(&common.load_program(&prg.program), &[&bclk_pin, &lrclk_pin]);
    cfg.set_out_pins(&[&data_pin]);

    cfg.shift_out = ShiftConfig {
        threshold: 32,
        direction: ShiftDirection::Left, // MSB first
        auto_fill: true,
    };
    // Joining the unused RX FIFO into TX gives an 8-word TX FIFO, smoothing DMA
    // bursts so the stream never starves mid-tone.
    cfg.fifo_join = FifoJoin::TxOnly;

    // State-machine clock = SAMPLE_RATE * CYCLES_PER_FRAME. clock_divider is
    // sys_clk / that. `ToFixed` gives the FixedU32<U8> the field expects.
    let sm_clk = SAMPLE_RATE * CYCLES_PER_FRAME;
    let div = embassy_rp::clocks::clk_sys_freq() as f32 / sm_clk as f32;
    cfg.clock_divider = div.to_fixed();

    sm.set_config(&cfg);
    // DATA + the two clock pins must be driven as outputs by the SM.
    sm.set_pin_dirs(Direction::Out, &[&data_pin, &bclk_pin, &lrclk_pin]);
    sm.set_enable(true);

    sm
}
