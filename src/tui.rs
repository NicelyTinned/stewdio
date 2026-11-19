//! Minimal terminal UI toolkit on top of crossterm: a cell buffer and a terminal guard.
use crossterm::{
    cursor,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, Write};

#[derive(Clone, Copy, PartialEq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
}

pub const fn st(fg: Color, bg: Color) -> Style {
    Style { fg, bg, bold: false }
}

pub fn bold(mut s: Style) -> Style {
    s.bold = true;
    s
}

pub const NORMAL: Style = st(Color::Reset, Color::Reset);
pub const DIM: Style = st(Color::DarkGrey, Color::Reset);
pub const BAR: Style = st(Color::Black, Color::Cyan);
pub const SEL: Style = st(Color::White, Color::Blue);
pub const SEL_DIM: Style = st(Color::White, Color::DarkGrey);
pub const BOX: Style = st(Color::White, Color::DarkBlue);
pub const BOX_SEL: Style = st(Color::Black, Color::Cyan);
pub const ERR: Style = st(Color::White, Color::DarkRed);

pub struct Screen {
    pub w: usize,
    pub h: usize,
    cells: Vec<(char, Style)>,
}

impl Screen {
    pub fn new(w: usize, h: usize) -> Screen {
        Screen { w, h, cells: vec![(' ', NORMAL); w * h] }
    }

    pub fn put(&mut self, x: usize, y: usize, ch: char, s: Style) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = (ch, s);
        }
    }

    /// Draw text, returns the number of columns used (clipped to `maxw`).
    pub fn text(&mut self, x: usize, y: usize, t: &str, s: Style, maxw: usize) -> usize {
        let mut n = 0;
        for ch in t.chars() {
            if n >= maxw {
                break;
            }
            self.put(x + n, y, ch, s);
            n += 1;
        }
        n
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, s: Style) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, ' ', s);
            }
        }
    }

    /// Draw a framed box with a title; returns the inner rectangle (x, y, w, h).
    pub fn boxed(&mut self, x: usize, y: usize, w: usize, h: usize, title: &str, s: Style) -> (usize, usize, usize, usize) {
        self.fill(x, y, w, h, s);
        for i in 0..w {
            self.put(x + i, y, '─', s);
            self.put(x + i, y + h - 1, '─', s);
        }
        for j in 0..h {
            self.put(x, y + j, '│', s);
            self.put(x + w - 1, y + j, '│', s);
        }
        self.put(x, y, '┌', s);
        self.put(x + w - 1, y, '┐', s);
        self.put(x, y + h - 1, '└', s);
        self.put(x + w - 1, y + h - 1, '┘', s);
        if !title.is_empty() {
            self.text(x + 2, y, &format!(" {title} "), bold(s), w.saturating_sub(4));
        }
        (x + 1, y + 1, w.saturating_sub(2), h.saturating_sub(2))
    }

    pub fn flush(&self, out: &mut impl Write) -> io::Result<()> {
        queue!(out, cursor::Hide)?;
        for y in 0..self.h {
            queue!(out, cursor::MoveTo(0, y as u16))?;
            let mut cur: Option<Style> = None;
            let mut buf = String::new();
            for x in 0..self.w {
                let (ch, s) = self.cells[y * self.w + x];
                if cur != Some(s) {
                    if !buf.is_empty() {
                        queue!(out, Print(&buf))?;
                        buf.clear();
                    }
                    queue!(
                        out,
                        SetForegroundColor(s.fg),
                        SetBackgroundColor(s.bg),
                        SetAttribute(if s.bold { Attribute::Bold } else { Attribute::NormalIntensity })
                    )?;
                    cur = Some(s);
                }
                buf.push(ch);
            }
            if !buf.is_empty() {
                queue!(out, Print(&buf))?;
            }
        }
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        out.flush()
    }
}

/// Puts the terminal in raw/alternate-screen mode and restores it on drop (even on panic).
pub struct Term;

impl Term {
    pub fn enter() -> io::Result<Term> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, cursor::Hide)?;
        Ok(Term)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            cursor::Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}
