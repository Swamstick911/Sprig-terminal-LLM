//! The twin-pad keyboard layout.
//!
//! 26 letters + 2 punctuation are split across **7 group buttons**; the 8th
//! button (`L`) is the space/action key. Selecting a letter is two taps:
//!
//! 1. tap the group button (one of [`GROUP_BUTTONS`]) → enter "letter" state,
//! 2. tap a left-pad button ([`LETTER_BUTTONS`]) to pick the letter within the
//!    group (index 0..4).
//!
//! In letter state the right-pad buttons ([`PREDICT_BUTTONS`]) instead accept a
//! predicted word (candidate 0..4).

use crate::button::Button;

/// The seven letter groups, indexed to match [`GROUP_BUTTONS`]. Each holds up to
/// four entries, mapped onto the left pad in order.
pub const GROUPS: [&str; 7] = [
    "abcd", "efgh", "ijkl", "mnop", "qrst", "uvwx", "yz.,",
];

/// Buttons that select a group in compose state (parallel to [`GROUPS`]).
pub const GROUP_BUTTONS: [Button; 7] = [
    Button::W,
    Button::A,
    Button::S,
    Button::D,
    Button::I,
    Button::J,
    Button::K,
];

/// Left-pad buttons that pick the letter within a group (index 0..4).
pub const LETTER_BUTTONS: [Button; 4] = [Button::W, Button::A, Button::S, Button::D];

/// Right-pad buttons that accept a predicted word (candidate 0..4).
pub const PREDICT_BUTTONS: [Button; 4] = [Button::I, Button::J, Button::K, Button::L];

/// Index of a group button within [`GROUP_BUTTONS`], if `b` selects a group.
pub fn group_index(b: Button) -> Option<usize> {
    GROUP_BUTTONS.iter().position(|&x| x == b)
}

/// Index of a letter-selection button within [`LETTER_BUTTONS`].
pub fn letter_index(b: Button) -> Option<usize> {
    LETTER_BUTTONS.iter().position(|&x| x == b)
}

/// Index of a prediction-accept button within [`PREDICT_BUTTONS`].
pub fn predict_index(b: Button) -> Option<usize> {
    PREDICT_BUTTONS.iter().position(|&x| x == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_26_letters_present_exactly_once() {
        let mut seen = [false; 26];
        for g in GROUPS {
            for c in g.chars() {
                if c.is_ascii_lowercase() {
                    let idx = (c as u8 - b'a') as usize;
                    assert!(!seen[idx], "letter {c} appears twice");
                    seen[idx] = true;
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "every letter a-z must be reachable");
    }

    #[test]
    fn groups_fit_the_left_pad() {
        for g in GROUPS {
            assert!(g.chars().count() <= LETTER_BUTTONS.len());
        }
    }

    #[test]
    fn group_and_letter_index_roundtrip() {
        assert_eq!(group_index(Button::S), Some(2));
        assert_eq!(letter_index(Button::D), Some(3));
        assert_eq!(predict_index(Button::I), Some(0));
        assert_eq!(predict_index(Button::L), Some(3));
        assert_eq!(group_index(Button::L), None);
    }
}
