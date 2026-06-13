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
    mono_font::{ascii::FONT_6X10, MonoTextStyle, MonoTextStyleBuilder},
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

        // Right side: CAPS flag + a live suggestion count (also a handy
        // indicator that prediction is firing).
        let mut right: String<12> = String::new();
        if kb.caps() {
            let _ = right.push_str("CAPS ");
        }
        let _ = write!(right, "s{}", kb.candidates().len());
        let w = right.len() as i32 * 6;
        Self::text(target, &right, WIDTH as i32 - w - 2, STATUS_TOP + 2, DIM)?;

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

        // Inline "ghost" completion (iOS QuickType / fish-shell style): the
        // not-yet-typed tail of the top suggestion, dimmed, right at the cursor.
        // Pressing space (L) accepts it.
        if let Some(suffix) = kb.completion_suffix() {
            let ghost = MonoTextStyle::new(&FONT_6X10, DIM);
            // A thin caret marks the boundary between typed text and the ghost.
            Rectangle::new(Point::new(x, y), Size::new(1, 9))
                .into_styled(PrimitiveStyle::with_fill(ACCENT))
                .draw(target)?;
            for ch in suffix.chars() {
                if col >= COLS {
                    x = 2;
                    y += LINE_H;
                    col = 0;
                }
                if y > GUIDE_TOP - LINE_H {
                    break;
                }
                buf.clear();
                let _ = buf.push(ch);
                Text::with_baseline(&buf, Point::new(x, y), ghost, Baseline::Top).draw(target)?;
                x += 6;
                col += 1;
            }
        } else if y <= GUIDE_TOP - LINE_H {
            // No completion: a solid block cursor.
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
            (68, GUIDE_TOP),       // W (up)
            (4, GUIDE_TOP + 11),   // A (left)
            (68, GUIDE_TOP + 22),  // S (down)
            (120, GUIDE_TOP + 11), // D (right)
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
        // One clear, fully-readable suggestion: pressing space (L) completes the
        // current word to it. Showing a single word avoids the old overlap.
        // The completion itself shows inline (ghost text) in the draft above;
        // here we just hint the accept key, or surface the top candidate so the
        // user always sees that prediction is working.
        if kb.completion_suffix().is_some() {
            Self::text(target, "space = accept word", 2, GUIDE_TOP, ACCENT)?;
        } else if let Some(top) = kb.candidates().first() {
            let mut s: String<30> = String::new();
            let _ = write!(s, "~ {}", top.as_str());
            Self::text(target, &s, 2, GUIDE_TOP, DIM)?;
        } else {
            Self::text(target, "L = space", 2, GUIDE_TOP, DIM)?;
        }

        // Lines 2-3: the group → button map (which button holds which letters).
        // Kept under 26 glyphs wide so it never runs off the 160px panel.
        Self::text(target, "Wabcd Aefgh Sijkl Dmnop", 0, GUIDE_TOP + 11, DIM)?;
        Self::text(target, "Iqrst Juvwx Kyz., L=spc", 0, GUIDE_TOP + 22, DIM)?;
        Ok(())
    }

    /// Full-screen "response view": a header banner plus wrapped body text.
    ///
    /// Milestone 2 uses this to show the streamed LLM reply. `header` is a short
    /// status line (e.g. "Streaming..." / "Done" / "Error"); `body` is the
    /// accumulated response text, word-naively wrapped to the panel width. Only
    /// the tail that fits on screen is shown (older text scrolls off the top),
    /// so the caller can pass an ever-growing buffer cheaply.
    pub fn response<D>(target: &mut D, header: &str, body: &str) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)?;
        Self::text(target, header, 2, STATUS_TOP + 2, ACCENT)?;
        Self::hline(target, DRAFT_TOP - 1, DIM)?;

        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        // Rows that fit between the header and the bottom edge.
        let max_rows = ((HEIGHT as i32 - (DRAFT_TOP + 2)) / LINE_H) as usize;
        let style = MonoTextStyle::new(&FONT_6X10, FG);

        // First pass: lay out into rows (char-wrap + honor '\n'), keeping only
        // the last `max_rows` so the freshest text is always visible.
        let mut rows: heapless::Vec<String<COLS>, 64> = heapless::Vec::new();
        let mut cur: String<COLS> = String::new();
        for ch in body.chars() {
            if ch == '\n' || cur.len() >= COLS {
                if rows.push(cur.clone()).is_err() {
                    rows.remove(0);
                    let _ = rows.push(cur.clone());
                }
                cur.clear();
                if ch == '\n' {
                    continue;
                }
            }
            let _ = cur.push(ch);
        }
        if rows.push(cur.clone()).is_err() {
            rows.remove(0);
            let _ = rows.push(cur);
        }

        let start = rows.len().saturating_sub(max_rows);
        let mut y = DRAFT_TOP + 2;
        for row in &rows[start..] {
            Text::with_baseline(row, Point::new(2, y), style, Baseline::Top).draw(target)?;
            y += LINE_H;
        }
        Ok(())
    }

    /// Like [`response`] but built for the streaming hot-loop — it never blanks
    /// the panel, so the screen doesn't flash between frames.
    ///
    /// [`response`] clears the whole screen (a ~40 KiB SPI fill) and then draws
    /// the text back on top; repeated every chunk while a reply streams, that
    /// blank-then-redraw is a visible flicker, and the big bus burst starves the
    /// WiFi task. This version instead overwrites in place: the header and every
    /// on-screen row are drawn with an *opaque* style (each glyph paints its own
    /// background) and padded to the full column width, so a shorter line erases
    /// whatever was under it. No clear, no flash, and a much smaller per-frame
    /// write. The caller should paint one clean [`response`] first (to set the
    /// margins) and then drive updates through this.
    pub fn response_stream<D>(target: &mut D, header: &str, body: &str) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        let max_rows = ((HEIGHT as i32 - (DRAFT_TOP + 2)) / LINE_H) as usize;

        let body_style = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(FG)
            .background_color(BG)
            .build();
        let header_style = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(ACCENT)
            .background_color(BG)
            .build();

        // Header, padded to full width so a shorter header overwrites the old one.
        let mut hdr: String<COLS> = String::new();
        for ch in header.chars() {
            if hdr.push(ch).is_err() {
                break;
            }
        }
        while hdr.len() < COLS && hdr.push(' ').is_ok() {}
        Text::with_baseline(&hdr, Point::new(2, STATUS_TOP + 2), header_style, Baseline::Top)
            .draw(target)?;
        Self::hline(target, DRAFT_TOP - 1, DIM)?;

        // Same tail-wrap as `response`: char-wrap, honor '\n', keep the last rows.
        let mut rows: heapless::Vec<String<COLS>, 64> = heapless::Vec::new();
        let mut cur: String<COLS> = String::new();
        for ch in body.chars() {
            if ch == '\n' || cur.len() >= COLS {
                if rows.push(cur.clone()).is_err() {
                    rows.remove(0);
                    let _ = rows.push(cur.clone());
                }
                cur.clear();
                if ch == '\n' {
                    continue;
                }
            }
            let _ = cur.push(ch);
        }
        if rows.push(cur.clone()).is_err() {
            rows.remove(0);
            let _ = rows.push(cur);
        }

        let start = rows.len().saturating_sub(max_rows);
        let visible = &rows[start..];
        let mut y = DRAFT_TOP + 2;
        // Draw a full grid of `max_rows` rows every time, padding each to the
        // column width and blanking any unused rows, so the body region is fully
        // repainted in place without ever clearing the screen.
        for i in 0..max_rows {
            let mut line: String<COLS> = String::new();
            if let Some(row) = visible.get(i) {
                for ch in row.chars() {
                    if line.push(ch).is_err() {
                        break;
                    }
                }
            }
            while line.len() < COLS && line.push(' ').is_ok() {}
            Text::with_baseline(&line, Point::new(2, y), body_style, Baseline::Top).draw(target)?;
            y += LINE_H;
        }
        Ok(())
    }

    /// Full-screen response view with a vertical scroll offset (in wrapped
    /// lines). Like [`response`], but instead of always showing the tail it
    /// renders a window starting at `scroll_lines` from the top of the wrapped
    /// body. The offset is clamped to the content so scrolling past the end is a
    /// no-op (the last screenful stays put). Used by the ShowingResponse mode so
    /// the user can read a long reply top-to-bottom with W/I (up) and S/K (down).
    pub fn response_scrolled<D>(
        target: &mut D,
        header: &str,
        body: &str,
        scroll_lines: usize,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)?;
        Self::text(target, header, 2, STATUS_TOP + 2, ACCENT)?;
        Self::hline(target, DRAFT_TOP - 1, DIM)?;

        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        let max_rows = ((HEIGHT as i32 - (DRAFT_TOP + 2)) / LINE_H) as usize;
        let style = MonoTextStyle::new(&FONT_6X10, FG);

        // Wrap the whole body into rows (char-wrap + honor '\n'). Keep a bounded
        // window of rows; the buffer holds the most recent 128 rows which is more
        // than the 2 KiB response can produce at 26 cols (~80 rows max).
        let mut rows: heapless::Vec<String<COLS>, 256> = heapless::Vec::new();
        let mut cur: String<COLS> = String::new();
        for ch in body.chars() {
            if ch == '\n' || cur.len() >= COLS {
                if rows.push(cur.clone()).is_err() {
                    rows.remove(0);
                    let _ = rows.push(cur.clone());
                }
                cur.clear();
                if ch == '\n' {
                    continue;
                }
            }
            let _ = cur.push(ch);
        }
        if rows.push(cur.clone()).is_err() {
            rows.remove(0);
            let _ = rows.push(cur);
        }

        // Clamp the start so the last screenful is the furthest we can scroll.
        let max_start = rows.len().saturating_sub(max_rows);
        let start = scroll_lines.min(max_start);
        let end = (start + max_rows).min(rows.len());

        let mut y = DRAFT_TOP + 2;
        for row in &rows[start..end] {
            Text::with_baseline(row, Point::new(2, y), style, Baseline::Top).draw(target)?;
            y += LINE_H;
        }
        Ok(())
    }

    /// Number of wrapped rows the body produces, and how many fit on one screen.
    /// The main loop uses this to clamp the scroll offset for
    /// [`response_scrolled`] without re-wrapping itself. Returns
    /// `(total_rows, visible_rows)`.
    pub fn wrapped_row_count(body: &str) -> (usize, usize) {
        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        let max_rows = ((HEIGHT as i32 - (DRAFT_TOP + 2)) / LINE_H) as usize;
        let mut rows: usize = 1;
        let mut col: usize = 0;
        for ch in body.chars() {
            if ch == '\n' || col >= COLS {
                rows += 1;
                col = 0;
                if ch == '\n' {
                    continue;
                }
            }
            col += 1;
        }
        (rows, max_rows)
    }

    /// A vertical settings menu: a title bar plus a list of items, with the
    /// `selected` row highlighted. Items are pre-formatted by the caller (e.g.
    /// "Model: deepseek/deepseek-chat") and truncated to the panel width.
    ///
    /// The list is taller than the screen (model/persona/tokens + quick prompts
    /// + games + back), so it scrolls: only a window of rows is drawn, kept
    /// around the cursor so the highlighted row is always visible, and `^`/`v`
    /// hints mark when there's more above or below. Used by the Settings mode.
    ///
    /// `sliders` is a slice of (row_index, current_value_0_10) for specialized
    /// rendering of numerical settings as progress bars.
    pub fn menu<D>(
        target: &mut D,
        title: &str,
        items: &[&str],
        selected: usize,
        sliders: &[(usize, u8)],
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(BG)?;
        // Title bar with background fill.
        Rectangle::new(Point::new(0, 0), Size::new(WIDTH, DRAFT_TOP as u32 - 1))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::CSS_DARK_SLATE_GRAY))
            .draw(target)?;
        Self::text(target, title, 2, STATUS_TOP + 2, FG)?;
        Self::hline(target, DRAFT_TOP - 1, DIM)?;

        const COLS: usize = (WIDTH as usize) / 6;
        const LINE_H: i32 = 11;
        let top = DRAFT_TOP + 3;
        let max_rows = ((HEIGHT as i32 - top) / LINE_H).max(1) as usize;

        // Scroll so the selected row stays on screen: center it in the window,
        // clamped so we never scroll past either end.
        let start = if items.len() <= max_rows {
            0
        } else {
            selected
                .saturating_sub(max_rows / 2)
                .min(items.len() - max_rows)
        };
        let end = (start + max_rows).min(items.len());

        let mut y = top;
        for i in start..end {
            let is_sel = i == selected;
            let color = if is_sel { ACCENT } else { FG };

            if is_sel {
                // Background highlight for the selected row.
                Rectangle::new(Point::new(0, y - 1), Size::new(WIDTH, LINE_H as u32))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::CSS_GRAY))
                    .draw(target)?;
            }

            // Build a "> item" / "  item" row.
            let mut row: String<COLS> = String::new();
            let _ = row.push_str(if is_sel { ">" } else { " " });
            for ch in items[i].chars() {
                if row.len() >= COLS - 1 {
                    break;
                }
                let _ = row.push(ch);
            }
            Self::text(target, &row, 2, y, if is_sel { BG } else { color })?;

            // If this row is a slider, draw a bar on the right.
            if let Some(&(_, val)) = sliders.iter().find(|(idx, _)| *idx == i) {
                let bar_w = 40u32;
                let bar_x = WIDTH as i32 - bar_w as i32 - 10;
                let fill_w = (bar_w * val as u32) / 10;
                
                // Track.
                Rectangle::new(Point::new(bar_x, y + 3), Size::new(bar_w, 4))
                    .into_styled(PrimitiveStyle::with_stroke(if is_sel { BG } else { DIM }, 1))
                    .draw(target)?;
                // Handle.
                if val > 0 {
                    Rectangle::new(Point::new(bar_x, y + 3), Size::new(fill_w, 4))
                        .into_styled(PrimitiveStyle::with_fill(if is_sel { BG } else { ACCENT }))
                        .draw(target)?;
                }
            }

            y += LINE_H;
        }

        // "more above / below" markers in the right margin.
        let hint_x = WIDTH as i32 - 8;
        if start > 0 {
            Self::text(target, "^", hint_x, top, DIM)?;
        }
        if end < items.len() {
            Self::text(target, "v", hint_x, top + (max_rows as i32 - 1) * LINE_H, DIM)?;
        }
        Ok(())
    }

    /// Action layer (after Hold L): label each button's action.
    fn draw_action_layer<D>(target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Self::text(target, "W=back A=send S=expand", 2, GUIDE_TOP, WARN)?;
        Self::text(target, "D=caps I=set J=newline", 2, GUIDE_TOP + 11, WARN)?;
        Self::text(target, "K=clear  L=cancel", 2, GUIDE_TOP + 22, WARN)?;
        Ok(())
    }
}
