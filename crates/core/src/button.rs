//! The eight physical buttons and the debounced events the keyboard consumes.
//!
//! Physical layout: a left D-pad (`W`/`A`/`S`/`D`) and a right cluster
//! (`I`/`J`/`K`/`L`). The input/hardware layer is responsible for debouncing and
//! hold-detection; it emits [`KeyEvent`]s that the [`crate::keyboard`] consumes.

/// One of the eight Sprig buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    W,
    A,
    S,
    D,
    I,
    J,
    K,
    L,
}

/// A debounced key event produced by the input layer.
///
/// `Tap` is a normal press-and-release; `Hold` fires when a button is held past
/// the hold threshold (used to reach the action layer via `Hold(L)`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyEvent {
    Tap(Button),
    Hold(Button),
}
