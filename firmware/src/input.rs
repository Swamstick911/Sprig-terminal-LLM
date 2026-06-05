//! Button scanning, debouncing, and hold-detection.
//!
//! The Sprig wires each button between its GPIO and ground, using the RP2040
//! internal pull-up, so an idle button reads **high** and a pressed button reads
//! **low**. This module polls all eight pins on a fixed tick, debounces each by
//! requiring the level to be stable for a few consecutive ticks, and turns the
//! resulting edges into [`KeyEvent`]s for the core keyboard:
//!
//!   * **Tap**: emitted on *release*, provided the press did not cross the hold
//!     threshold. Releasing on the next compose/letter selection feels instant
//!     because the keyboard logic only cares about discrete events.
//!   * **Hold**: emitted *once* while still pressed, when a button has been held
//!     past [`HOLD_TICKS`]. Only `L` uses this today (arms the action layer),
//!     but the scanner emits it for any button so the core can decide.
//!
//! Emitting `Tap` on release (not press) lets us suppress the trailing `Tap`
//! after a `Hold` fired — exactly what the keyboard wants for `Hold(L)`.

use embassy_rp::gpio::{Input, Pull};
use sprig_llm_core::button::{Button, KeyEvent};

/// Poll period for the scan loop. ~5 ms gives snappy response while leaving the
/// CPU idle the rest of the time under Embassy.
pub const TICK_MS: u64 = 5;

/// Number of consecutive stable samples required to accept a level change.
/// 3 * 5 ms = 15 ms of debounce, comfortably past typical tactile-switch bounce.
pub const DEBOUNCE_TICKS: u8 = 3;

/// Ticks a button must stay down before a `Hold` is emitted. 100 * 5 ms ≈ 0.5 s.
pub const HOLD_TICKS: u16 = 100;

/// The eight buttons in a fixed scan order, paired with their [`Button`] id.
pub const SCAN_ORDER: [Button; 8] = [
    Button::W,
    Button::A,
    Button::S,
    Button::D,
    Button::I,
    Button::J,
    Button::K,
    Button::L,
];

/// Per-button debounce + hold state.
#[derive(Clone, Copy)]
struct Debounced {
    /// Last *stable* (debounced) pressed state.
    pressed: bool,
    /// Candidate raw level we are currently counting toward stability.
    candidate: bool,
    /// How many consecutive ticks `candidate` has held.
    stable_for: u8,
    /// Ticks the (debounced) press has been held; 0 when released.
    held_ticks: u16,
    /// Whether a `Hold` has already been emitted for the current press.
    hold_fired: bool,
}

impl Debounced {
    const fn new() -> Self {
        Self {
            pressed: false,
            candidate: false,
            stable_for: 0,
            held_ticks: 0,
            hold_fired: false,
        }
    }
}

/// What a single debounce step produced for one button this tick.
enum Edge {
    None,
    /// A debounced release that should become a `Tap` (hold did not fire).
    Tap,
    /// A debounced hold crossing `HOLD_TICKS` for the first time this press.
    Hold,
}

impl Debounced {
    /// Feed one raw sample (`raw_pressed` = pin reads low). Returns any edge.
    fn step(&mut self, raw_pressed: bool) -> Edge {
        // Debounce: count consecutive identical raw samples.
        if raw_pressed == self.candidate {
            if self.stable_for < DEBOUNCE_TICKS {
                self.stable_for += 1;
            }
        } else {
            self.candidate = raw_pressed;
            self.stable_for = 1;
        }

        // Commit the candidate once it has been stable long enough.
        if self.stable_for >= DEBOUNCE_TICKS && self.candidate != self.pressed {
            self.pressed = self.candidate;
            if self.pressed {
                // Fresh press: start the hold timer.
                self.held_ticks = 0;
                self.hold_fired = false;
            } else {
                // Release: emit a Tap only if we never crossed the hold line.
                let was_hold = self.hold_fired;
                self.held_ticks = 0;
                self.hold_fired = false;
                if !was_hold {
                    return Edge::Tap;
                }
                return Edge::None;
            }
        }

        // While held, advance the hold timer and fire Hold exactly once.
        if self.pressed {
            if self.held_ticks < u16::MAX {
                self.held_ticks += 1;
            }
            if !self.hold_fired && self.held_ticks >= HOLD_TICKS {
                self.hold_fired = true;
                return Edge::Hold;
            }
        }

        Edge::None
    }
}

/// Owns the eight input pins and their debounce state.
///
/// Pins are type-erased to [`Input`] over `AnyPin` by passing already-degraded
/// pins in; see [`Buttons::new`].
pub struct Buttons<'d> {
    pins: [Input<'d>; 8],
    state: [Debounced; 8],
}

impl<'d> Buttons<'d> {
    /// Build the scanner from eight configured-but-raw input pins.
    ///
    /// Each pin must already be an [`Input`] with [`Pull::Up`]. The order MUST
    /// match [`SCAN_ORDER`] (W, A, S, D, I, J, K, L). See [`from_pins`] for a
    /// helper that configures the pulls for you.
    pub fn new(pins: [Input<'d>; 8]) -> Self {
        Self {
            pins,
            state: [Debounced::new(); 8],
        }
    }

    /// Poll all eight buttons once and invoke `emit` for every event produced
    /// this tick. Call once per [`TICK_MS`].
    pub fn poll(&mut self, mut emit: impl FnMut(KeyEvent)) {
        for idx in 0..8 {
            // Pressed == pin low (button ties GPIO to GND through a pull-up).
            let raw_pressed = self.pins[idx].is_low();
            match self.state[idx].step(raw_pressed) {
                Edge::None => {}
                Edge::Tap => emit(KeyEvent::Tap(SCAN_ORDER[idx])),
                Edge::Hold => emit(KeyEvent::Hold(SCAN_ORDER[idx])),
            }
        }
    }
}

/// Convenience: take eight raw `AnyPin`s (in [`SCAN_ORDER`]) and configure each
/// as input-with-pull-up. Keeps [`crate::main`] tidy.
pub fn configure(pins: [embassy_rp::gpio::AnyPin; 8]) -> [Input<'static>; 8] {
    pins.map(|p| Input::new(p, Pull::Up))
}

#[cfg(test)]
mod tests {
    // NOTE: the debounce/hold logic in `Debounced` is pure and host-testable,
    // but the surrounding `Input`/`AnyPin` types come from embassy-rp and only
    // build for the device target. The keyboard interaction is covered by the
    // core crate's tests; here we document the intended edge behavior.
    //
    // Press → (DEBOUNCE_TICKS lows) → committed pressed.
    // Release before HOLD_TICKS  → Edge::Tap.
    // Held past HOLD_TICKS       → Edge::Hold once, release yields no Tap.
}
