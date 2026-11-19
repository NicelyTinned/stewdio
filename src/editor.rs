//! Full-screen script editor with Luau syntax highlighting.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crossterm::style::Color;

use crate::luau::{self, Carry, Tok};
use crate::tui::{self, st, Screen, Style};

pub enum EditorEvent {
    None,
    Exit,
    Save,
}

pub struct Editor {
    pub lines: Vec<String>,
    pub title: String,
    pub modified: bool,
    cy: usize,
    cx: usize,
    top: usize,
    left: usize,
    want_col: usize,
    undo: Vec<(Vec<String>, usize, usize)>,
    diags: Vec<luau::Diag>,
    find: Option<String>,
    last_find: String,
    crlf: bool,
    msg: String,
    view_h: usize,
    gutter: usize,
}

fn byte_idx(line: &str, ci: usize) -> usize {
    line.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(line.len())
}

fn clen(line: &str) -> usize {
    line.chars().count()
}

fn tok_style(t: Tok) -> Style {
    match t {
        Tok::Plain => tui::NORMAL,
        Tok::Keyword => st(Color::Magenta, Color::Reset),
        Tok::Builtin => st(Color::Cyan, Color::Reset),
        Tok::Str => st(Color::Green, Color::Reset),
        Tok::Number => st(Color::Yellow, Color::Reset),
        Tok::Comment => st(Color::DarkGrey, Color::Reset),
        Tok::Op => st(Color::DarkYellow, Color::Reset),
    }
}

impl Editor {
    pub fn new(title: &str, src: &str) -> Editor {
        let crlf = src.contains("\r\n");
        let norm = src.replace("\r\n", "\n");
        let mut lines: Vec<String> = norm.split('\n').map(|l| l.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let diags = luau::diagnostics(&lines);
        Editor {
            lines,
            title: title.to_string(),
            modified: false,
            cy: 0,
            cx: 0,
            top: 0,
            left: 0,
            want_col: 0,
            undo: Vec::new(),
            diags,
            find: None,
            last_find: String::new(),
            crlf,
            msg: String::new(),
            view_h: 10,
            gutter: 5,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join(if self.crlf { "\r\n" } else { "\n" })
    }

    pub fn mark_saved(&mut self) {
        self.modified = false;
        self.msg = "Applied to the instance (Ctrl+S in the explorer writes it to disk)".to_string();
    }

    fn snapshot(&mut self) {
        self.undo.push((self.lines.clone(), self.cy, self.cx));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }

    fn changed(&mut self) {
        self.modified = true;
        self.diags = luau::diagnostics(&self.lines);
    }

    fn clamp(&mut self) {
        if self.cy >= self.lines.len() {
            self.cy = self.lines.len() - 1;
        }
        let l = clen(&self.lines[self.cy]);
        if self.cx > l {
            self.cx = l;
        }
    }

    fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            if ch == '\n' {
                self.newline(false);
            } else if ch != '\r' {
                let b = byte_idx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert(b, ch);
                self.cx += 1;
            }
        }
        self.want_col = self.cx;
    }

    fn newline(&mut self, auto_indent: bool) {
        let b = byte_idx(&self.lines[self.cy], self.cx);
        let rest = self.lines[self.cy].split_off(b);
        let mut indent: String = if auto_indent {
            self.lines[self.cy].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
        } else {
            String::new()
        };
        if auto_indent {
            let t = self.lines[self.cy].trim_end();
            let opens = t.ends_with("then") || t.ends_with("do") || t.ends_with("else") || t.ends_with("repeat")
                || t.ends_with(')') && t.contains("function")
                || t.ends_with('{');
            if opens {
                indent.push('\t');
            }
        }
        let n = clen(&indent);
        self.lines.insert(self.cy + 1, format!("{indent}{rest}"));
        self.cy += 1;
        self.cx = n;
        self.want_col = self.cx;
    }

    fn move_vert(&mut self, d: isize) {
        let ny = (self.cy as isize + d).clamp(0, self.lines.len() as isize - 1) as usize;
        self.cy = ny;
        self.cx = self.want_col.min(clen(&self.lines[ny]));
    }

    fn word_left(&mut self) {
        if self.cx == 0 {
            if self.cy > 0 {
                self.cy -= 1;
                self.cx = clen(&self.lines[self.cy]);
            }
            return;
        }
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i > 0 && !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_' {
            i -= 1;
        }
        while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
            i -= 1;
        }
        self.cx = i;
    }

    fn word_right(&mut self) {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        if self.cx >= chars.len() {
            if self.cy + 1 < self.lines.len() {
                self.cy += 1;
                self.cx = 0;
            }
            return;
        }
        let mut i = self.cx;
        while i < chars.len() && !(chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        self.cx = i;
    }

    fn find_next(&mut self) {
        if self.last_find.is_empty() {
            return;
        }
        let needle = self.last_find.clone();
        let total = self.lines.len();
        for off in 0..=total {
            let y = (self.cy + off) % total;
            let line = &self.lines[y];
            let start_c = if off == 0 { self.cx + 1 } else { 0 };
            let sb = byte_idx(line, start_c.min(clen(line)));
            if let Some(pos) = line[sb..].find(&needle) {
                let cb = sb + pos;
                self.cy = y;
                self.cx = line[..cb].chars().count();
                self.want_col = self.cx;
                self.msg = format!("Found '{needle}'");
                return;
            }
        }
        self.msg = format!("'{needle}' not found");
    }

    pub fn on_paste(&mut self, s: &str) {
        if let Some(f) = &mut self.find {
            f.push_str(&s.replace('\n', ""));
            return;
        }
        self.snapshot();
        self.insert_str(s);
        self.changed();
    }

    pub fn on_key(&mut self, k: KeyEvent) -> EditorEvent {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.find.is_some() {
            match k.code {
                KeyCode::Esc => self.find = None,
                KeyCode::Enter => {
                    if let Some(f) = self.find.take() {
                        if !f.is_empty() {
                            self.last_find = f;
                        }
                    }
                    self.find_next();
                }
                KeyCode::Backspace => {
                    if let Some(f) = &mut self.find {
                        f.pop();
                    }
                }
                KeyCode::Char(c) if !ctrl => {
                    if let Some(f) = &mut self.find {
                        f.push(c);
                    }
                }
                _ => {}
            }
            return EditorEvent::None;
        }
        self.msg.clear();
        match (k.code, ctrl) {
            (KeyCode::Char('x'), true) => return EditorEvent::Exit,
            (KeyCode::Char('s'), true) => return EditorEvent::Save,
            (KeyCode::Char('z'), true) => {
                if let Some((l, y, x)) = self.undo.pop() {
                    self.lines = l;
                    self.cy = y;
                    self.cx = x;
                    self.changed();
                }
            }
            (KeyCode::Char('f'), true) => self.find = Some(String::new()),
            (KeyCode::F(3), _) => self.find_next(),
            (KeyCode::Char('k'), true) => {
                self.snapshot();
                if self.lines.len() > 1 {
                    self.lines.remove(self.cy);
                } else {
                    self.lines[0].clear();
                }
                self.clamp();
                self.changed();
            }
            (KeyCode::Char('d'), true) => {
                self.snapshot();
                let l = self.lines[self.cy].clone();
                self.lines.insert(self.cy + 1, l);
                self.cy += 1;
                self.changed();
            }
            (KeyCode::Left, true) => self.word_left(),
            (KeyCode::Right, true) => self.word_right(),
            (KeyCode::Up, _) => self.move_vert(-1),
            (KeyCode::Down, _) => self.move_vert(1),
            (KeyCode::PageUp, _) => self.move_vert(-(self.view_h as isize)),
            (KeyCode::PageDown, _) => self.move_vert(self.view_h as isize),
            (KeyCode::Left, _) => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = clen(&self.lines[self.cy]);
                }
                self.want_col = self.cx;
            }
            (KeyCode::Right, _) => {
                if self.cx < clen(&self.lines[self.cy]) {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
                self.want_col = self.cx;
            }
            (KeyCode::Home, _) => {
                let first = self.lines[self.cy].chars().take_while(|c| c.is_whitespace()).count();
                self.cx = if self.cx == first { 0 } else { first };
                self.want_col = self.cx;
            }
            (KeyCode::End, _) => {
                self.cx = clen(&self.lines[self.cy]);
                self.want_col = self.cx;
            }
            (KeyCode::Enter, _) => {
                self.snapshot();
                self.newline(true);
                self.changed();
            }
            (KeyCode::Tab, _) => {
                self.snapshot();
                self.insert_str("\t");
                self.changed();
            }
            (KeyCode::Backspace, _) => {
                self.snapshot();
                if self.cx > 0 {
                    let b = byte_idx(&self.lines[self.cy], self.cx - 1);
                    self.lines[self.cy].remove(b);
                    self.cx -= 1;
                } else if self.cy > 0 {
                    let cur = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = clen(&self.lines[self.cy]);
                    self.lines[self.cy].push_str(&cur);
                }
                self.want_col = self.cx;
                self.changed();
            }
            (KeyCode::Delete, _) => {
                self.snapshot();
                if self.cx < clen(&self.lines[self.cy]) {
                    let b = byte_idx(&self.lines[self.cy], self.cx);
                    self.lines[self.cy].remove(b);
                } else if self.cy + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].push_str(&next);
                }
                self.changed();
            }
            (KeyCode::Char(c), false) => {
                self.snapshot();
                self.insert_str(&c.to_string());
                self.changed();
            }
            _ => {}
        }
        EditorEvent::None
    }

    fn display_starts(line: &str) -> (Vec<usize>, usize) {
        let mut starts = Vec::new();
        let mut col = 0;
        for ch in line.chars() {
            starts.push(col);
            if ch == '\t' {
                col += 4 - col % 4;
            } else {
                col += 1;
            }
        }
        (starts, col)
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.top = self.top.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                self.top = (self.top + 3).min(self.lines.len().saturating_sub(1));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = (m.column as usize, m.row as usize);
                if y >= 1 && y <= self.view_h && x >= self.gutter {
                    let ly = (self.top + y - 1).min(self.lines.len() - 1);
                    let dcol = x - self.gutter + self.left;
                    let (starts, _) = Self::display_starts(&self.lines[ly]);
                    let mut ci = starts.len();
                    for (i, s) in starts.iter().enumerate() {
                        if *s >= dcol {
                            ci = i;
                            break;
                        }
                    }
                    self.cy = ly;
                    self.cx = ci.min(clen(&self.lines[ly]));
                    self.want_col = self.cx;
                }
            }
            _ => {}
        }
    }

    pub fn draw(&mut self, scr: &mut Screen) {
        let (w, h) = (scr.w, scr.h);
        if w < 20 || h < 6 {
            scr.text(0, 0, "Terminal too small", tui::NORMAL, w);
            return;
        }
        self.view_h = h - 3;
        self.gutter = (self.lines.len().to_string().len() + 2).max(4);
        let text_w = w.saturating_sub(self.gutter);
        // keep cursor in view
        if self.cy < self.top {
            self.top = self.cy;
        }
        if self.cy >= self.top + self.view_h {
            self.top = self.cy + 1 - self.view_h;
        }
        let (starts, _) = Self::display_starts(&self.lines[self.cy]);
        let cursor_col = starts.get(self.cx).copied().unwrap_or_else(|| Self::display_starts(&self.lines[self.cy]).1);
        if cursor_col < self.left {
            self.left = cursor_col;
        }
        if cursor_col >= self.left + text_w {
            self.left = cursor_col + 1 - text_w;
        }

        scr.fill(0, 0, w, 1, tui::BAR);
        let title = format!(" Script editor: {}{}", self.title, if self.modified { " [modified]" } else { "" });
        scr.text(0, 0, &title, tui::bold(tui::BAR), w);

        let mut carry = Carry::None;
        let end = (self.top + self.view_h).min(self.lines.len());
        for ly in 0..end {
            let chars: Vec<char> = self.lines[ly].chars().collect();
            let (toks, next) = luau::lex_line(&chars, carry);
            carry = next;
            if ly < self.top {
                continue;
            }
            let y = 1 + ly - self.top;
            let num_style = if ly == self.cy { st(Color::Yellow, Color::Reset) } else { tui::DIM };
            let has_diag = self.diags.iter().any(|d| d.line == ly);
            let num = format!("{:>w$} ", ly + 1, w = self.gutter - 1);
            scr.text(0, y, &num, num_style, self.gutter);
            if has_diag {
                scr.put(0, y, '!', st(Color::Red, Color::Reset));
            }
            let mut col = 0usize;
            for (i, ch) in chars.iter().enumerate() {
                let s = tok_style(toks[i]);
                let width = if *ch == '\t' { 4 - col % 4 } else { 1 };
                for k in 0..width {
                    let dc = col + k;
                    if dc >= self.left && dc - self.left < text_w {
                        let c = if *ch == '\t' { ' ' } else { *ch };
                        scr.put(self.gutter + dc - self.left, y, c, s);
                    }
                }
                col += width;
            }
        }
        // cursor
        let cy_screen = 1 + self.cy - self.top;
        let cx_screen = self.gutter + cursor_col - self.left;
        let under = self.lines[self.cy].chars().nth(self.cx).filter(|c| *c != '\t').unwrap_or(' ');
        scr.put(cx_screen, cy_screen, under, st(Color::Black, Color::White));

        // diagnostics + status
        let diag = match self.diags.first() {
            Some(d) => format!(" ! line {}: {}", d.line + 1, d.msg),
            None => " No problems found".to_string(),
        };
        let dstyle = if self.diags.is_empty() { tui::DIM } else { st(Color::Yellow, Color::Reset) };
        scr.fill(0, h - 2, w, 1, dstyle);
        scr.text(0, h - 2, &diag, dstyle, w);
        scr.fill(0, h - 1, w, 1, tui::BAR);
        let help = if let Some(f) = &self.find {
            format!(" Find: {f}_   (Enter = search, Esc = cancel)")
        } else if !self.msg.is_empty() {
            format!(" {}", self.msg)
        } else {
            " Ctrl+X exit  Ctrl+S apply  Ctrl+Z undo  Ctrl+F find  F3 next  Ctrl+K del line  Ctrl+D dup".to_string()
        };
        scr.text(0, h - 1, &help, tui::BAR, w);
        let pos = format!("Ln {}, Col {} ", self.cy + 1, self.cx + 1);
        if w > pos.len() + help.chars().count() {
            scr.text(w - pos.len(), h - 1, &pos, tui::BAR, pos.len());
        }
    }
}
