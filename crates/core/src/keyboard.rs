//! The twin-pad keyboard state machine — the heart of the project.
//!
//! Text entry is two taps per letter and fully deterministic (no wrong-word
//! guessing):
//!
//! * **Compose** (default): the seven group buttons each open a letter group;
//!   `L` types a space; `Hold(L)` arms the action layer.
//! * **Letter**: the left pad picks the letter within the chosen group; the
//!   right pad accepts a predicted word.
//! * **Action layer** (after `Hold(L)`): the next button is Backspace / Send /
//!   Expand / Caps / Symbols / Newline / Clear.
//!
//! Prediction is layered on top: after each committed letter the keyboard asks
//! the [`Predictor`] for candidates, which the right pad can accept in letter
//! state.

use crate::button::{Button, KeyEvent};
use crate::layout;
use crate::predict::{Candidates, Predictor};
use heapless::String;

/// Maximum draft length (bytes).
pub const BUF: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Compose,
    /// Picking a letter within the group at this [`layout::GROUPS`] index.
    Letter(usize),
}

/// What the app should do after a key event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Nothing changed; no redraw needed.
    Idle,
    /// Visible state changed; redraw the screen.
    Redraw,
    /// User asked to send the draft to the LLM.
    Send,
    /// User asked to expand the draft shorthand via the LLM.
    Expand,
}

/// The keyboard. Owns the draft buffer, caps state, and current candidates.
pub struct Keyboard {
    buf: String<BUF>,
    state: State,
    /// Set after `Hold(L)`; the next event is interpreted as an action.
    action_armed: bool,
    /// One-shot capitalization for the next typed letter.
    caps: bool,
    cands: Candidates,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    pub const fn new() -> Self {
        Self {
            buf: String::new(),
            state: State::Compose,
            action_armed: false,
            caps: false,
            cands: Candidates::new(),
        }
    }

    /// Current draft text.
    pub fn text(&self) -> &str {
        &self.buf
    }

    /// Current prediction candidates.
    pub fn candidates(&self) -> &Candidates {
        &self.cands
    }

    /// Whether the next letter will be capitalized.
    pub fn caps(&self) -> bool {
        self.caps
    }

    /// Whether the action layer is armed (for the renderer's hint row).
    pub fn action_armed(&self) -> bool {
        self.action_armed
    }

    /// While picking a letter, the active group's letters mapped onto the left
    /// pad in order (index 0 → W, 1 → A, 2 → S, 3 → D). `None` in compose state.
    ///
    /// The renderer uses this to show the "pick a letter" step.
    pub fn active_group(&self) -> Option<&'static str> {
        match self.state {
            State::Letter(g) => Some(layout::GROUPS[g]),
            State::Compose => None,
        }
    }

    /// Process one key event, returning what the app should do.
    pub fn process(&mut self, ev: KeyEvent, predictor: &dyn Predictor) -> Outcome {
        if self.action_armed {
            return self.handle_action(ev, predictor);
        }
        match ev {
            KeyEvent::Hold(Button::L) => {
                self.action_armed = true;
                Outcome::Redraw
            }
            // Holds on other buttons are unused in v1.
            KeyEvent::Hold(_) => Outcome::Idle,
            KeyEvent::Tap(b) => self.handle_tap(b, predictor),
        }
    }

    fn handle_tap(&mut self, b: Button, predictor: &dyn Predictor) -> Outcome {
        match self.state {
            State::Compose => {
                if b == Button::L {
                    self.push_char(' ');
                    self.repredict(predictor);
                    return Outcome::Redraw;
                }
                if let Some(g) = layout::group_index(b) {
                    self.state = State::Letter(g);
                    return Outcome::Redraw;
                }
                Outcome::Idle
            }
            State::Letter(g) => {
                if let Some(i) = layout::letter_index(b) {
                    if let Some(ch) = layout::GROUPS[g].chars().nth(i) {
                        let ch = if self.caps {
                            ch.to_ascii_uppercase()
                        } else {
                            ch
                        };
                        self.push_char(ch);
                        self.caps = false; // one-shot
                        self.state = State::Compose;
                        self.repredict(predictor);
                        return Outcome::Redraw;
                    }
                    // Group has fewer than 4 letters and this slot is empty.
                    self.state = State::Compose;
                    return Outcome::Redraw;
                }
                if let Some(pi) = layout::predict_index(b) {
                    if let Some(word) = self.cands.get(pi).cloned() {
                        self.accept_word(word.as_str());
                        self.state = State::Compose;
                        self.repredict(predictor);
                        return Outcome::Redraw;
                    }
                    // No candidate in that slot → cancel back to compose.
                    self.state = State::Compose;
                    return Outcome::Redraw;
                }
                Outcome::Idle
            }
        }
    }

    fn handle_action(&mut self, ev: KeyEvent, predictor: &dyn Predictor) -> Outcome {
        self.action_armed = false;
        let b = match ev {
            KeyEvent::Tap(b) | KeyEvent::Hold(b) => b,
        };
        match b {
            Button::W => {
                self.backspace();
                self.repredict(predictor);
                Outcome::Redraw
            }
            Button::A => Outcome::Send,
            Button::S => Outcome::Expand,
            Button::D => {
                self.caps = !self.caps;
                Outcome::Redraw
            }
            Button::J => {
                self.push_char('\n');
                self.repredict(predictor);
                Outcome::Redraw
            }
            Button::K => {
                self.buf.clear();
                self.repredict(predictor);
                Outcome::Redraw
            }
            // I = symbols/numbers layer (v2 placeholder); L = released w/o action.
            Button::I => Outcome::Redraw,
            Button::L => Outcome::Idle,
        }
    }

    fn push_char(&mut self, ch: char) {
        let _ = self.buf.push(ch);
    }

    fn backspace(&mut self) {
        self.buf.pop();
    }

    /// Byte offset where the current (in-progress) word begins.
    fn prefix_start(&self) -> usize {
        match self.buf.rfind(|c: char| c == ' ' || c == '\n') {
            Some(i) => i + 1,
            None => 0,
        }
    }

    fn repredict(&mut self, predictor: &dyn Predictor) {
        let start = self.prefix_start();
        let mut tmp = Candidates::new();
        predictor.predict(&self.buf[start..], &mut tmp);
        self.cands = tmp;
    }

    /// Replace the current partial word with `word` and a trailing space.
    ///
    /// If the replacement would not fit in the draft buffer, the draft is left
    /// unchanged rather than partially rewritten.
    fn accept_word(&mut self, word: &str) {
        let start = self.prefix_start();
        let kept = start; // bytes before the partial word
        if kept + word.len() + 1 > BUF {
            return; // won't fit — leave the draft intact
        }
        while self.buf.len() > start {
            self.buf.pop();
        }
        let _ = self.buf.push_str(word);
        let _ = self.buf.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::Button::*;
    use crate::predict::StaticPredictor;

    fn tap(k: &mut Keyboard, b: Button, p: &StaticPredictor) -> Outcome {
        k.process(KeyEvent::Tap(b), p)
    }
    fn hold(k: &mut Keyboard, b: Button, p: &StaticPredictor) -> Outcome {
        k.process(KeyEvent::Hold(b), p)
    }

    #[test]
    fn types_hi_in_two_taps_each() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        // 'h' is in group A (efgh), index 3 → tap A, tap D
        tap(&mut k, A, &p);
        tap(&mut k, D, &p);
        assert_eq!(k.text(), "h");
        // 'i' is in group S (ijkl), index 0 → tap S, tap W
        tap(&mut k, S, &p);
        tap(&mut k, W, &p);
        assert_eq!(k.text(), "hi");
    }

    #[test]
    fn space_via_l() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        tap(&mut k, W, &p); // group abcd
        tap(&mut k, W, &p); // 'a'
        assert_eq!(tap(&mut k, L, &p), Outcome::Redraw);
        assert_eq!(k.text(), "a ");
    }

    #[test]
    fn caps_is_one_shot() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        hold(&mut k, L, &p); // arm action
        tap(&mut k, D, &p); // D = caps toggle
        assert!(k.caps());
        tap(&mut k, W, &p); // group abcd
        tap(&mut k, W, &p); // 'A'
        tap(&mut k, A, &p); // group efgh
        tap(&mut k, W, &p); // 'e'
        assert_eq!(k.text(), "Ae");
    }

    #[test]
    fn backspace_via_action_layer() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        tap(&mut k, W, &p);
        tap(&mut k, W, &p); // 'a'
        tap(&mut k, W, &p);
        tap(&mut k, A, &p); // 'b'
        assert_eq!(k.text(), "ab");
        hold(&mut k, L, &p); // arm
        tap(&mut k, W, &p); // W = backspace
        assert_eq!(k.text(), "a");
    }

    #[test]
    fn send_and_expand_actions() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        hold(&mut k, L, &p);
        assert_eq!(tap(&mut k, A, &p), Outcome::Send);
        hold(&mut k, L, &p);
        assert_eq!(tap(&mut k, S, &p), Outcome::Expand);
    }

    #[test]
    fn accept_prediction_inserts_word_and_space() {
        let p = StaticPredictor::new(&["hi", "hello", "help"]);
        let mut k = Keyboard::new();
        // type 'h' → candidates for "h" = [hi, hello, help]
        tap(&mut k, A, &p);
        tap(&mut k, D, &p);
        assert_eq!(k.text(), "h");
        assert_eq!(k.candidates().len(), 3);
        // enter a letter state, then accept candidate index 1 ("hello") via J
        tap(&mut k, S, &p);
        tap(&mut k, J, &p);
        assert_eq!(k.text(), "hello ");
    }

    #[test]
    fn accepting_a_word_that_would_overflow_leaves_draft_intact() {
        // A 20-char candidate that won't fit once the draft is nearly full.
        let p = StaticPredictor::new(&["aardvarkaardvarkaard"]);
        let mut k = Keyboard::new();
        // Fill the draft with "a " pairs up to 248 bytes.
        for _ in 0..124 {
            tap(&mut k, W, &p); // group abcd
            tap(&mut k, W, &p); // 'a'
            tap(&mut k, L, &p); // space
        }
        // Start a fresh partial word "a"; candidates now hold the long word.
        tap(&mut k, W, &p);
        tap(&mut k, W, &p); // 'a'
        let before = heapless::String::<{ BUF }>::try_from(k.text()).unwrap();
        assert_eq!(k.candidates().len(), 1);
        // Enter letter state and try to accept the (too-long) candidate.
        tap(&mut k, S, &p);
        tap(&mut k, I, &p); // accept candidate 0 → must no-op, not corrupt
        assert_eq!(k.text(), before.as_str());
    }

    #[test]
    fn active_group_reflects_state() {
        let p = StaticPredictor::new(&[]);
        let mut k = Keyboard::new();
        assert_eq!(k.active_group(), None);
        tap(&mut k, A, &p); // enter letter state for group "efgh"
        assert_eq!(k.active_group(), Some("efgh"));
        tap(&mut k, D, &p); // commit 'h' → back to compose
        assert_eq!(k.active_group(), None);
    }

    #[test]
    fn prediction_uses_only_current_word() {
        let p = StaticPredictor::new(&["world"]);
        let mut k = Keyboard::new();
        // "a " then start "w"
        tap(&mut k, W, &p);
        tap(&mut k, W, &p); // 'a'
        tap(&mut k, L, &p); // space
        tap(&mut k, J, &p); // group uvwx
        tap(&mut k, S, &p); // index2 → 'w'
        assert_eq!(k.text(), "a w");
        assert_eq!(k.candidates().len(), 1);
        assert_eq!(k.candidates()[0].as_str(), "world");
    }
}
