//! Insert finalized history lines into the terminal's **normal scrollback**,
//! above the inline viewport. Ported and trimmed from codex-rs
//! `tui/src/insert_history.rs` (Standard / DECSTBM path only).
//!
//! # The mechanism (codex's "Standard" mode)
//!
//! The inline viewport occupies the bottom `viewport_area.height` rows. To add
//! a finalized line *above* it without repainting the viewport, we:
//!
//! 1. Set a DECSTBM scroll region covering the rows above the viewport
//!    (`CSI 1 ; top r`).
//! 2. Move to the bottom of that region and emit Reverse Index (`ESC M`) to
//!    scroll the region's content down, opening blank rows — but when there is
//!    room below the viewport we instead let the region above grow.
//! 3. Print the new line(s) into the freed space.
//! 4. Reset the scroll region (`CSI r`) and restore the cursor.
//!
//! Because the printed rows land in the terminal's own grid (not ratatui's
//! double buffer), they become **real scrollback**: the user can mouse-select
//! them, scroll to them with the wheel / scrollbar, and copy them via tmux
//! copy-mode — all with no app mode key. That is the whole point.
//!
//! We pre-wrap each [`Line`] to the viewport width so a long line occupies the
//! right number of physical rows. The wrapping here is a straightforward
//! word-wrap (codex additionally keeps URLs unsplit; that refinement is
//! deferred — see the crate-level notes).

use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Color as CColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::backend::Backend;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::tui_terminal::Terminal;

/// `CSI top ; bottom r` — set the DECSTBM scroll region (1-based, inclusive).
fn set_scroll_region<W: Write>(w: &mut W, top: u16, bottom: u16) -> io::Result<()> {
    write!(w, "\x1b[{top};{bottom}r")
}

/// `CSI r` — reset the scroll region to the full screen.
fn reset_scroll_region<W: Write>(w: &mut W) -> io::Result<()> {
    write!(w, "\x1b[r")
}

/// Insert `lines` into scrollback above the inline viewport, sliding the
/// viewport down by as much as fits below it (and scrolling older history up
/// otherwise). Updates `terminal.viewport_area` so the next draw paints in the
/// viewport's new position. Cursor-position-neutral: restores the cursor to
/// where it was on entry.
pub fn insert_history_lines<B>(terminal: &mut Terminal<B>, lines: Vec<Line>) -> io::Result<()>
where
    B: Backend + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));
    let mut area = terminal.viewport_area;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let wrap_width = area.width.max(1) as usize;

    // Pre-wrap to the viewport width so each physical row is one grid row.
    let mut wrapped: Vec<Line> = Vec::new();
    for line in &lines {
        wrapped.extend(wrap_line(line, wrap_width));
    }
    let wrapped_rows = wrapped.len() as u16;
    if wrapped_rows == 0 {
        return Ok(());
    }

    let writer = terminal.backend_mut();
    let mut should_update_area = false;

    // If there is room below the viewport, grow the region above by scrolling
    // the area between the screen top and the viewport bottom; the viewport then
    // shifts down. Otherwise the region above is already full-height and the
    // reverse-index scroll pushes the oldest rows into scrollback.
    let cursor_top = if area.bottom() < screen_size.height {
        let scroll_amount = wrapped_rows.min(screen_size.height - area.bottom());
        let top_1based = area.top() + 1;
        set_scroll_region(writer, top_1based, screen_size.height)?;
        queue!(writer, MoveTo(0, area.top()))?;
        for _ in 0..scroll_amount {
            queue!(writer, Print("\x1bM"))?; // Reverse Index (ESC M): scroll region down.
        }
        reset_scroll_region(writer)?;
        let cursor_top = area.top().saturating_sub(1);
        area.y += scroll_amount;
        should_update_area = true;
        cursor_top
    } else {
        area.top().saturating_sub(1)
    };

    // Limit scrolling to the rows above the viewport, then print the new lines
    // starting just below the previous top of that region.
    set_scroll_region(writer, 1, area.top())?;
    queue!(writer, MoveTo(0, cursor_top))?;
    for line in &wrapped {
        queue!(writer, Print("\r\n"))?;
        write_history_line(writer, line)?;
    }
    reset_scroll_region(writer)?;

    // Restore the cursor to where it was (history insertion is position-neutral).
    queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

    if should_update_area {
        terminal.set_viewport_area(area);
    }
    // Flush these out-of-band scrollback writes now. The draw() that follows
    // only flushes the backend when the live viewport diff or the cursor
    // changed, so without this the inserted history could sit buffered and not
    // appear until some later write (codex P2). This only runs when there is new
    // history to insert, so an idle TUI still emits nothing.
    Backend::flush(terminal.backend_mut())?;
    Ok(())
}

/// Word-wrap a [`Line`] to `width` display columns, preserving per-span style.
/// Empty lines round up to one physical row.
fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![owned_line(line)];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_width = 0usize;

    for span in &line.spans {
        // Split the span into words while keeping whitespace runs attached so
        // wrapping doesn't drop the spacing inside a styled run.
        for word in split_keep_ws(span.content.as_ref()) {
            let w = word.width();
            if cur_width + w > width && cur_width > 0 {
                out.push(finish_line(std::mem::take(&mut cur), line.style));
                cur_width = 0;
                if word.trim().is_empty() {
                    continue; // don't start a wrapped row with leading whitespace
                }
            }
            // A single word longer than the width: hard-split it.
            if w > width {
                for chunk in hard_split(word, width) {
                    let cw = chunk.width();
                    if cur_width + cw > width && cur_width > 0 {
                        out.push(finish_line(std::mem::take(&mut cur), line.style));
                        cur_width = 0;
                    }
                    cur.push(Span::styled(chunk.to_string(), span.style));
                    cur_width += cw;
                }
            } else {
                cur.push(Span::styled(word.to_string(), span.style));
                cur_width += w;
            }
        }
    }
    if !cur.is_empty() {
        out.push(finish_line(cur, line.style));
    }
    if out.is_empty() {
        out.push(Line::default().style(line.style));
    }
    out
}

fn finish_line(spans: Vec<Span<'static>>, style: ratatui::style::Style) -> Line<'static> {
    Line::from(spans).style(style)
}

fn owned_line(line: &Line) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|s| Span::styled(s.content.to_string(), s.style))
        .collect::<Vec<_>>();
    Line::from(spans).style(line.style)
}

/// Split a string into words and whitespace runs (both preserved).
fn split_keep_ws(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev_ws: Option<bool> = None;
    for (i, ch) in s.char_indices() {
        let is_ws = ch.is_whitespace();
        if let Some(p) = prev_ws
            && p != is_ws
        {
            out.push(&s[start..i]);
            start = i;
        }
        prev_ws = Some(is_ws);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Hard-split an over-long word into `width`-column chunks (char boundaries).
fn hard_split(word: &str, width: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut col = 0usize;
    for (i, ch) in word.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + cw > width && i > start {
            out.push(&word[start..i]);
            start = i;
            col = 0;
        }
        col += cw;
    }
    if start < word.len() {
        out.push(&word[start..]);
    }
    if out.is_empty() {
        out.push(word);
    }
    out
}

/// Write a single (already-wrapped) history line: set colors, clear to EOL,
/// then write each styled span. Caller positions the cursor / emits `\r\n`.
fn write_history_line<W: Write>(writer: &mut W, line: &Line) -> io::Result<()> {
    queue!(
        writer,
        SetColors(Colors::new(
            line.style.fg.map(Into::into).unwrap_or(CColor::Reset),
            line.style.bg.map(Into::into).unwrap_or(CColor::Reset),
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;
    // Merge the line-level style into each span so ANSI colors reflect it.
    let merged: Vec<Span> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged.iter())
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            queue_modifier_diff(&mut writer, last_modifier, modifier)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(next_fg.into(), next_bg.into()))
            )?;
            fg = next_fg;
            bg = next_bg;
        }
        queue!(writer, Print(span.content.clone()))?;
    }
    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

fn queue_modifier_diff<W: Write>(w: &mut W, from: Modifier, to: Modifier) -> io::Result<()> {
    use crossterm::style::Attribute as A;
    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        queue!(w, SetAttribute(A::NoReverse))?;
    }
    if removed.contains(Modifier::BOLD) {
        queue!(w, SetAttribute(A::NormalIntensity))?;
        if to.contains(Modifier::DIM) {
            queue!(w, SetAttribute(A::Dim))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(w, SetAttribute(A::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(w, SetAttribute(A::NoUnderline))?;
    }
    if removed.contains(Modifier::DIM) {
        queue!(w, SetAttribute(A::NormalIntensity))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(w, SetAttribute(A::NotCrossedOut))?;
    }

    let added = to - from;
    if added.contains(Modifier::REVERSED) {
        queue!(w, SetAttribute(A::Reverse))?;
    }
    if added.contains(Modifier::BOLD) {
        queue!(w, SetAttribute(A::Bold))?;
    }
    if added.contains(Modifier::ITALIC) {
        queue!(w, SetAttribute(A::Italic))?;
    }
    if added.contains(Modifier::UNDERLINED) {
        queue!(w, SetAttribute(A::Underlined))?;
    }
    if added.contains(Modifier::DIM) {
        queue!(w, SetAttribute(A::Dim))?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        queue!(w, SetAttribute(A::CrossedOut))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::{Backend, ClearType as RtClearType, WindowSize};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::Style;

    /// A `Backend + Write` that records every byte emitted, so tests can assert on
    /// the exact escape-sequence stream `insert_history_lines` writes into the
    /// terminal's real scrollback (codex's tests use a VT100 backend for this; the
    /// octos-tui crate has no vt100 dep, so we inspect the raw bytes instead).
    struct RecordingBackend {
        buf: Vec<u8>,
        size: Size,
    }

    impl RecordingBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                buf: Vec::new(),
                size: Size::new(width, height),
            }
        }

        fn output(&self) -> String {
            String::from_utf8_lossy(&self.buf).into_owned()
        }
    }

    impl Write for RecordingBackend {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for RecordingBackend {
        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Ok(())
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(Position { x: 0, y: 0 })
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, _position: P) -> io::Result<()> {
            Ok(())
        }
        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clear_region(&mut self, _clear_type: RtClearType) -> io::Result<()> {
            Ok(())
        }
        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }
        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.size,
                pixels: Size::new(0, 0),
            })
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn term(width: u16, height: u16) -> Terminal<RecordingBackend> {
        let mut t = Terminal::new(RecordingBackend::new(width, height)).expect("terminal");
        // Anchor a 1-row viewport at the bottom so history inserts scroll upward.
        t.set_viewport_area(Rect::new(0, height - 1, width, 1));
        t
    }

    /// `crossterm::style::SetBackgroundColor(Reset)` byte sequence (`CSI 49 m`).
    /// Any background SGR that is NOT this is a non-default (theme) background.
    fn default_bg_seq() -> String {
        let mut bytes: Vec<u8> = Vec::new();
        queue!(bytes, SetBackgroundColor(CColor::Reset)).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn history_line_with_theme_bg_is_emitted_on_default_background() {
        // Bug 3 (b): a finalized line that *carried* a theme surface background
        // would emit `SetBackgroundColor(Rgb(..))` and a `Clear(UntilNewLine)`
        // under it, painting a "brown block" that bleeds to the row's right edge.
        // After the fix, finalized lines have no bg (Bug 3a strips it), so the
        // only background SGR in the scrollback stream is the default reset.
        let mut t = term(40, 6);
        // Mirror a stripped finalized line: fg set, NO bg (default background).
        let line = Line::from(vec![Span::styled(
            "committed reply",
            Style::default().fg(Color::Rgb(236, 239, 244)),
        )]);
        insert_history_lines(&mut t, vec![line]).expect("insert history");

        let out = t.backend().output();
        let default_bg = default_bg_seq();
        // Any non-default background SGR in the stream means a theme surface
        // leaked into scrollback; only the default reset (`CSI 49 m`) is allowed.
        assert!(
            !out.contains("\x1b[48;2;"),
            "scrollback stream emitted a truecolor background (brown block): {out:?}"
        );
        assert!(
            !out.contains("\x1b[48;5;"),
            "scrollback stream emitted an indexed background (brown block): {out:?}"
        );
        assert!(
            out.contains(&default_bg),
            "scrollback stream should reset the background to default: {out:?}"
        );
    }

    #[test]
    fn history_write_resets_sgr_after_each_line() {
        // Bug 3 (b): un-reset SGR would bleed the last line's colors into the
        // scroll-region ops / subsequent prints. Every history write must end on
        // a full reset (fg + bg + attributes) so nothing bleeds "all over".
        let mut t = term(40, 6);
        let line = Line::from(vec![Span::styled(
            "styled",
            Style::default()
                .fg(Color::Rgb(110, 188, 255))
                .add_modifier(Modifier::BOLD),
        )]);
        insert_history_lines(&mut t, vec![line]).expect("insert history");

        let out = t.backend().output();
        // crossterm emits `SetAttribute(Reset)` as `CSI 0 m`.
        assert!(
            out.contains("\x1b[0m"),
            "expected an SGR reset (CSI 0 m) in the scrollback stream: {out:?}"
        );
        // The reset trio must appear; the foreground reset is `CSI 39 m`.
        assert!(
            out.contains("\x1b[39m"),
            "expected a foreground reset (CSI 39 m) in the scrollback stream: {out:?}"
        );
    }

    #[test]
    fn wrap_short_line_is_unchanged() {
        let line = Line::from("hello");
        let out = wrap_line(&line, 20);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].width(), 5);
    }

    #[test]
    fn wrap_splits_on_word_boundary() {
        let line = Line::from("the quick brown fox jumps");
        let out = wrap_line(&line, 10);
        assert!(out.len() >= 3, "expected wrapping, got {out:?}");
        for l in &out {
            assert!(l.width() <= 10, "row too wide: {l:?}");
        }
    }

    #[test]
    fn wrap_hard_splits_overlong_word() {
        let line = Line::from("abcdefghijklmnopqrstuvwxyz");
        let out = wrap_line(&line, 8);
        assert!(out.len() >= 3);
        for l in &out {
            assert!(l.width() <= 8);
        }
    }

    #[test]
    fn wrap_preserves_span_style() {
        use ratatui::style::Stylize;
        let line = Line::from(vec!["bold ".bold(), "and plain text here".into()]);
        let out = wrap_line(&line, 8);
        assert!(out.len() >= 2);
        // The first span's bold style must survive on the first row.
        let first_has_bold = out[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(first_has_bold, "bold style lost: {out:?}");
    }

    #[test]
    fn split_keep_ws_round_trips() {
        let s = "a  bc   d";
        let parts = split_keep_ws(s);
        assert_eq!(parts.concat(), s);
    }
}
