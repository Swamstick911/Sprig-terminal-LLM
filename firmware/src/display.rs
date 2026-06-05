//! The ST7735 LCD and the four-zone keyboard renderer.
//!
//! The Sprig screen is a 160x128 ST7735. We drive it over `SPI0` with the
//! `st7735-lcd` driver, which exposes an embedded-graphics [`DrawTarget`] in
//! `Rgb565`. On top of that we lay out four horizontal zones that mirror the
//! [`Keyboard`] state:
//!
//! ```text
//! +------------------------------------------+  y=0
//! | STATUS         caps?  action?            |   status bar
//! +------------------------------------------+  y=14
//! |                                          |
//! | draft text (the message being composed)  |   draft (wraps)
//! |                                          |
//! +------------------------------------------+  y=92
//! | pred: word1  word2  word3  word4         |   prediction row (I/J/K/L)
//! +------------------------------------------+  y=110
//! | hint: group letters / action labels      |   group/action hint
//! +------------------------------------------+  y=128
//! ```
//!
//! The renderer is deliberately allocation-free and `no_std`: all strings are
//! built into fixed `heapless` buffers.

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
use sprig_llm_core::layout;

/// Physical panel size.
pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 128;

// Zone boundaries (top y of each band).
const STATUS_TOP: i32 = 0;
const DRAFT_TOP: i32 = 14;
const PRED_TOP: i32 = 92;
const HINT_TOP: i32 = 110;

// A small palette.
const BG: Rgb565 = Rgb565::BLACK;
const FG: Rgb565 = Rgb565::WHITE;
const ACCENT: Rgb565 = Rgb565::CSS_DODGER_BLUE;
const DIM: Rgb565 = Rgb565::CSS_DIM_GRAY;
const WARN: Rgb565 = Rgb565::CSS_ORANGE;

/// A short, transient status message shown in the status bar (e.g. "SENDING").
pub type Status = String<24>;

/// Renders [`Keyboard`] state to any embedded-graphics target.
///
/// Generic over the target so it can be unit-tested against a mock display on
/// the host if desired, and used with the real `ST7735` on the device.
pub struct Ui;

impl Ui {
    /// Clear the whole screen to the background color.
    pub fn clear<D>(target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)
    }

    /// Redraw all four zones from the current keyboard state.
    ///
    /// `status` is an optional banner (Milestone 1 uses it for Send/Expand
    /// placeholders); pass an empty string for the default "READY".
    pub fn render<D>(
        target: &mut D,
        kb: &Keyboard,
        status: &str,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)?;
        Self::draw_status(target, kb, status)?;
        Self::draw_draft(target, kb)?;
        Self::draw_predictions(target, kb)?;
        Self::draw_hint(target, kb)?;
        Ok(())
    }

    fn text<D>(
        target: &mut D,
        s: &str,
        x: i32,
        y: i32,
        color: Rgb565,
    ) -> Result<(), D::Error>
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

    /// Status bar: a label plus caps / action-armed indicators.
    fn draw_status<D>(
        target: &mut D,
        kb: &Keyboard,
        status: &str,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let label = if status.is_empty() { "READY" } else { status };
        let color = if status.is_empty() { ACCENT } else { WARN };
        Self::text(target, label, 2, STATUS_TOP + 2, color)?;

        // Right-aligned flags: "CAPS" and "ACT" when active.
        let mut flags: String<16> = String::new();
        if kb.caps() {
            let _ = flags.push_str("CAPS ");
        }
        if kb.action_armed() {
            let _ = flags.push_str("ACT");
        }
        if !flags.is_empty() {
            // 6 px per glyph in FONT_6X10; right-align within the bar.
            let w = flags.len() as i32 * 6;
            Self::text(target, &flags, WIDTH as i32 - w - 2, STATUS_TOP + 2, WARN)?;
        }
        Self::hline(target, DRAFT_TOP - 1, DIM)?;
        Ok(())
    }

    /// Draft text zone with naive word-pixel wrapping and a trailing cursor.
    fn draw_draft<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        const COLS: usize = (WIDTH as usize) / 6; // glyphs per line
        const LINE_H: i32 = 11;
        let text = kb.text();

        let mut x = 2i32;
        let mut y = DRAFT_TOP + 2;
        let mut col = 0usize;

        // Render char-by-char so we can wrap at the column limit and honor '\n'.
        let style = MonoTextStyle::new(&FONT_6X10, FG);
        let mut buf: String<2> = String::new();
        for ch in text.chars() {
            if ch == '\n' || col >= COLS {
                x = 2;
                y += LINE_H;
                col = 0;
                if ch == '\n' {
                    continue;
                }
            }
            if y > PRED_TOP - LINE_H {
                break; // out of draft space; stop drawing
            }
            buf.clear();
            let _ = buf.push(ch);
            Text::with_baseline(&buf, Point::new(x, y), style, Baseline::Top)
                .draw(target)?;
            x += 6;
            col += 1;
        }

        // Block cursor at the current insertion point.
        if y <= PRED_TOP - LINE_H {
            Rectangle::new(Point::new(x, y), Size::new(6, 9))
                .into_styled(PrimitiveStyle::with_fill(ACCENT))
                .draw(target)?;
        }
        Ok(())
    }

    /// Prediction row: up to four candidates, labeled by their accept button.
    fn draw_predictions<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Self::hline(target, PRED_TOP - 1, DIM)?;
        let cands = kb.candidates();
        if cands.is_empty() {
            Self::text(target, "(no suggestions)", 2, PRED_TOP + 2, DIM)?;
            return Ok(());
        }
        // Four quarter columns, each tagged with its right-pad button letter.
        let labels = ["I", "J", "K", "L"];
        let colw = WIDTH as i32 / 4;
        for (i, c) in cands.iter().enumerate().take(4) {
            let x = i as i32 * colw + 2;
            let mut line: String<28> = String::new();
            let _ = write!(line, "{}:{}", labels[i], c.as_str());
            Self::text(target, &line, x, PRED_TOP + 2, FG)?;
        }
        Ok(())
    }

    /// Bottom hint row: in compose state, list the group letters under each
    /// group button; when the action layer is armed, show the action labels.
    fn draw_hint<D>(target: &mut D, kb: &Keyboard) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Self::hline(target, HINT_TOP - 1, DIM)?;
        if kb.action_armed() {
            // Mirrors keyboard::handle_action: W=Bksp A=Send S=Expand D=Caps
            // I=Sym J=NL K=Clr L=cancel.
            Self::text(
                target,
                "W<x A>send S>exp D>CAP",
                2,
                HINT_TOP + 1,
                WARN,
            )?;
            Self::text(target, "I=sym J=nl K=clr L=esc", 2, HINT_TOP + 10, WARN)?;
            return Ok(());
        }

        // Compose hint: show the seven groups joined, e.g. "abcd efgh ijkl ...".
        let mut line: String<48> = String::new();
        for (i, g) in layout::GROUPS.iter().enumerate() {
            if i > 0 {
                let _ = line.push(' ');
            }
            let _ = line.push_str(g);
        }
        Self::text(target, &line, 2, HINT_TOP + 1, DIM)?;
        Self::text(target, "L=space  hold L=actions", 2, HINT_TOP + 10, DIM)?;
        Ok(())
    }
}
