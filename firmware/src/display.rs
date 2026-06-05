//! The ST7735 LCD and the keyboard renderer.
//!
//! The Sprig screen is a 160x128 ST7735, driven over `SPI0` with the
//! `st7735-lcd` driver (an embedded-graphics `DrawTarget` in `Rgb565`). The
//! layout mirrors the [`Keyboard`] state and, crucially, guides the user
//! through the two-tap entry method:
//!
//! ```text
//! +------------------------------------------+  y=0
//! | <step prompt>                 CAPS        |   status bar
//! +------------------------------------------+  y=14
//! | draft text (the message being composed)  |   draft (wraps)
//! +------------------------------------------+  y=92
//! | step 1 (compose): suggestions + the      |
//! |   group->button map                      |   guidance zone
//! | step 2 (letter):  the chosen group's     |
//! |   letters arranged like the D-pad        |
//! +------------------------------------------+  y=128
//! ```
//!
//! The renderer is allocation-free and `no_std`: all strings are built into
//! fixed `heapless` buffers.

use core::fmt::Write as _;

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;
use sprig_llm_core::keyboard::Keyboard;

/// Physical panel size.
pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 128;

// Zone boundaries (top y of each band).
const STATUS_TOP: i32 = 0;
const DRAFT_TOP: i32 = 14;
const GUIDE_TOP: i32 = 92;

// A small palette.
const BG: Rgb565 = Rgb565::BLACK;
const FG: Rgb565 = Rgb565::WHITE;
const ACCENT: Rgb565 = Rgb565::CSS_DODGER_BLUE;
const DIM: Rgb565 = Rgb565::CSS_DIM_GRAY;
const WARN: Rgb565 = Rgb565::CSS_ORANGE;

/// A short, transient status message (e.g. "SENDING").
pub type Status = String<24>;

/// Renders [`Keyboard`] state to any embedded-graphics target.
pub struct Ui;

impl Ui {
    /// Clear the whole screen to the background color.
    pub fn clear<D>(target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)
    }

    /// Redraw every zone from the current keyboard state.
    ///
    /// `status` is an optional banner (Send/Expand placeholders); pass an empty
    /// string for the default step prompt.
    pub fn render<D>(target: &mut D, kb: &Keyboard, status: &str) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)?;
        Self::draw_status(target, kb, status)?;
        Self::draw_draft(target, kb)?;
        Self::draw_guide(target, kb)?;
        Ok(())
    }

    fn text<D>(target: &mut D, s: &str, x: i32, y: i32, color: Rgb565) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let style = MonoTextStyle::new(&FONT_6X10, color);
        Text::with_baseline(s, Point::new(x, y), style, Baseline::Top).draw(target)?;
        Ok(())
    }

    fn hline<D>(target: &mut D, y: i32, color: Rgb565) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Rectangle::new(Point::new(0, y), Size::new(WIDTH, 1))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(target)?;
        Ok(())
    }

    /// Status bar: a step prompt that tells the user exactly what to do next,
    /// plus a CAPS indicator.
    fn draw_status<D>(target: &mut D, kb: &Keyboard, status: &str) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let (label, color) = if !status.is_empty() {
            (status, WARN)
        } else if kb.action_armed() {
            ("ACTION: pick one", WARN)
        } else if kb.active_group().is_some() {
            ("STEP 2: pick the letter", ACCENT)
        } else {
            ("STEP 1: tap a group", ACCENT)
        };
        Self::text(target, label, 2, STATUS_TOP + 2, color)?;

        if kb.caps() {
            Self::text(target, "CAPS", WIDTH as i32 - 4 * 6 - 2, STATUS_TOP + 2, WARN)?;
        }
        Self::hline(target, DRAFT_TOP - 1, DIM)?;
        Ok(())
    }

    /// Draft text zone with naive per-character wrapping and a block cursor.
    fn draw_draft<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        let style = MonoTextStyle::new(&FONT_6X10, FG);

        let mut x = 2i32;
        let mut y = DRAFT_TOP + 2;
        let mut col = 0usize;
        let mut buf: String<2> = String::new();

        for ch in kb.text().chars() {
            if ch == '\n' || col >= COLS {
                x = 2;
                y += LINE_H;
                col = 0;
                if ch == '\n' {
                    continue;
                }
            }
            if y > GUIDE_TOP - LINE_H {
                break;
            }
            buf.clear();
            let _ = buf.push(ch);
            Text::with_baseline(&buf, Point::new(x, y), style, Baseline::Top).draw(target)?;
            x += 6;
            col += 1;
        }

        if y <= GUIDE_TOP - LINE_H {
            Rectangle::new(Point::new(x, y), Size::new(6, 9))
                .into_styled(PrimitiveStyle::with_fill(ACCENT))
                .draw(target)?;
        }
        Ok(())
    }

    /// The guidance zone — the part that makes typing self-explanatory. It
    /// branches on the keyboard's current step.
    fn draw_guide<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Self::hline(target, GUIDE_TOP - 1, DIM)?;

        if kb.action_armed() {
            return Self::draw_action_layer(target);
        }
        match kb.active_group() {
            Some(letters) => Self::draw_letter_cross(target, letters),
            None => Self::draw_compose_guide(target, kb),
        }
    }

    /// Step 2: show the chosen group's letters arranged like the physical left
    /// D-pad, so the user just presses the matching direction.
    ///
    /// ```text
    ///            W:e          (up)
    ///   A:f               D:h (left / right)
    ///            S:g          (down)
    /// ```
    fn draw_letter_cross<D>(target: &mut D, letters: &str) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Button label paired with the group letter at that index.
        let cells = [("W", 0usize), ("A", 1), ("S", 2), ("D", 3)];
        // (x, y) for up / left / down / right within the guide band.
        let pos = [
            (68, GUIDE_TOP + 1),  // W (up)
            (4, GUIDE_TOP + 13),  // A (left)
            (68, GUIDE_TOP + 25), // S (down)
            (120, GUIDE_TOP + 13), // D (right)
        ];
        for (i, (btn, idx)) in cells.iter().enumerate() {
            if let Some(ch) = letters.chars().nth(*idx) {
                let mut cell: String<8> = String::new();
                let _ = write!(cell, "{}:{}", btn, ch);
                let (x, y) = pos[i];
                Self::text(target, &cell, x, y, ACCENT)?;
            }
        }
        Ok(())
    }

    /// Step 1: suggestions (acceptable from the right pad) plus the
    /// group→button map so the user knows which button to tap.
    fn draw_compose_guide<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Line 1: predictions, each tagged with its right-pad accept button.
        let cands = kb.candidates();
        if cands.is_empty() {
            Self::text(target, "type for suggestions", 2, GUIDE_TOP + 1, DIM)?;
        } else {
            let labels = ["I", "J", "K", "L"];
            let colw = WIDTH as i32 / 4;
            for (i, c) in cands.iter().enumerate().take(4) {
                let mut s: String<20> = String::new();
                let _ = write!(s, "{}:{}", labels[i], c.as_str());
                Self::text(target, &s, i as i32 * colw + 1, GUIDE_TOP + 1, FG)?;
            }
        }

        // Lines 2-3: the group → button map (which button holds which letters).
        Self::text(target, "W:abcd A:efgh S:ijkl D:mnop", 0, GUIDE_TOP + 13, DIM)?;
        Self::text(target, "I:qrst J:uvwx K:yz.,  L=space", 0, GUIDE_TOP + 25, DIM)?;
        Ok(())
    }

    /// Action layer (after Hold L): label each button's action.
    fn draw_action_layer<D>(target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Self::text(target, "W=back A=send S=expand", 2, GUIDE_TOP + 1, WARN)?;
        Self::text(target, "D=caps I=sym J=newline", 2, GUIDE_TOP + 13, WARN)?;
        Self::text(target, "K=clear  L=cancel", 2, GUIDE_TOP + 25, WARN)?;
        Ok(())
    }
}
