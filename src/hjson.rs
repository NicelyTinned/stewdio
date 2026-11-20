//! Small HJSON reader/writer (the subset Stewdio needs).
//! Reader: comments (#, //, /* */), optional commas, quoted/unquoted keys,
//! quoteless strings (to end of line). Writer: comma-less, always-quoted.

use crate::util::is_json_number;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Raw numeric literal, preserved exactly.
    Number(String),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        if let Value::Object(o) = self {
            o.iter().find(|(k, _)| k.as_str() == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        if let Value::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }
}

pub fn parse(src: &str) -> Result<Value, String> {
    let src = src.trim_start_matches('\u{feff}');
    let mut p = P { c: src.chars().collect(), i: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.c.len() {
        return Err(p.err("unexpected trailing content"));
    }
    Ok(v)
}

struct P {
    c: Vec<char>,
    i: usize,
}

impl P {
    fn err(&self, m: &str) -> String {
        let mut line = 1;
        let mut col = 1;
        for ch in &self.c[..self.i.min(self.c.len())] {
            if *ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        format!("{m} (line {line}, column {col})")
    }

    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn skip_line(&mut self) {
        while self.i < self.c.len() && self.c[self.i] != '\n' {
            self.i += 1;
        }
    }

    fn ws(&mut self) {
        loop {
            match self.peek() {
                Some(ch) if ch.is_whitespace() || ch == ',' => self.i += 1,
                Some('#') => self.skip_line(),
                Some('/') if self.c.get(self.i + 1) == Some(&'/') => self.skip_line(),
                Some('/') if self.c.get(self.i + 1) == Some(&'*') => {
                    self.i += 2;
                    while self.i < self.c.len()
                        && !(self.c[self.i] == '*' && self.c.get(self.i + 1) == Some(&'/'))
                    {
                        self.i += 1;
                    }
                    self.i = (self.i + 2).min(self.c.len());
                }
                _ => break,
            }
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') | Some('\'') => {
                if self.c.get(self.i..self.i + 3).map(|s| s == ['\'', '\'', '\'']).unwrap_or(false) {
                    return Err(self.err("multiline ''' strings are not supported"));
                }
                let s = self.quoted()?;
                Ok(Value::Str(s))
            }
            Some(_) => self.scalar(),
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                None => return Err(self.err("unterminated object")),
                Some('}') => {
                    self.i += 1;
                    break;
                }
                _ => {}
            }
            let key = self.key()?;
            self.ws();
            if self.peek() != Some(':') {
                return Err(self.err("expected ':' after key"));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            items.push((key, v));
        }
        Ok(Value::Object(items))
    }

    fn array(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                None => return Err(self.err("unterminated array")),
                Some(']') => {
                    self.i += 1;
                    break;
                }
                _ => {}
            }
            items.push(self.value()?);
        }
        Ok(Value::Array(items))
    }

    fn key(&mut self) -> Result<String, String> {
        match self.peek() {
            Some('"') | Some('\'') => self.quoted(),
            _ => {
                let st = self.i;
                while let Some(ch) = self.peek() {
                    if ch == ':' || ch.is_whitespace() || matches!(ch, ',' | '{' | '}' | '[' | ']') {
                        break;
                    }
                    self.i += 1;
                }
                if self.i == st {
                    return Err(self.err("expected a key"));
                }
                Ok(self.c[st..self.i].iter().collect())
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.c.len() {
            return Err(self.err("truncated \\u escape"));
        }
        let t: String = self.c[self.i..self.i + 4].iter().collect();
        self.i += 4;
        match u32::from_str_radix(&t, 16) {
            Ok(v) => Ok(v),
            Err(_) => Err(self.err("invalid \\u escape")),
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        let q = self.c[self.i];
        self.i += 1;
        let mut s = String::new();
        loop {
            let ch = match self.c.get(self.i) {
                Some(c) => *c,
                None => return Err(self.err("unterminated string")),
            };
            self.i += 1;
            if ch == q {
                break;
            }
            if ch == '\\' {
                let e = match self.c.get(self.i) {
                    Some(c) => *c,
                    None => return Err(self.err("unterminated escape")),
                };
                self.i += 1;
                match e {
                    '"' => s.push('"'),
                    '\'' => s.push('\''),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    'u' => {
                        let cp = self.hex4()?;
                        if (0xD800..0xDC00).contains(&cp)
                            && self.c.get(self.i) == Some(&'\\')
                            && self.c.get(self.i + 1) == Some(&'u')
                        {
                            let save = self.i;
                            self.i += 2;
                            let lo = self.hex4()?;
                            if (0xDC00..0xE000).contains(&lo) {
                                let full = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                s.push(char::from_u32(full).unwrap_or('\u{FFFD}'));
                            } else {
                                self.i = save;
                                s.push('\u{FFFD}');
                            }
                        } else {
                            s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                    }
                    _ => return Err(self.err("invalid escape sequence")),
                }
            } else {
                s.push(ch);
            }
        }
        Ok(s)
    }

    fn scalar(&mut self) -> Result<Value, String> {
        let st = self.i;
        let mut e = st;
        while e < self.c.len()
            && !(self.c[e].is_whitespace() || matches!(self.c[e], ',' | '}' | ']' | '#'))
        {
            e += 1;
        }
        let tok: String = self.c[st..e].iter().collect();
        let mut k = e;
        while k < self.c.len() && (self.c[k] == ' ' || self.c[k] == '\t' || self.c[k] == '\r') {
            k += 1;
        }
        let delim_ok = k >= self.c.len()
            || matches!(self.c[k], '\n' | ',' | '}' | ']' | '#')
            || (self.c[k] == '/' && self.c.get(k + 1) == Some(&'/'));
        if delim_ok && !tok.is_empty() {
            let v = match tok.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                "null" => Some(Value::Null),
                t if is_json_number(t) => Some(Value::Number(t.to_string())),
                _ => None,
            };
            if let Some(v) = v {
                self.i = e;
                return Ok(v);
            }
        }
        // quoteless string: runs to the end of the line
        let mut eol = st;
        while eol < self.c.len() && self.c[eol] != '\n' {
            eol += 1;
        }
        let line: String = self.c[st..eol].iter().collect();
        self.i = eol;
        Ok(Value::Str(line.trim().to_string()))
    }
}

// ---------------------------------------------------------------- writer

pub fn to_string(v: &Value) -> String {
    let mut s = String::new();
    write_value(v, 0, &mut s);
    s.push('\n');
    s
}

fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

fn write_quoted(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn key_is_plain(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_alphanumeric() || matches!(c, '_' | '$' | '@') => {}
        _ => return false,
    }
    k.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '@' | '-' | '.'))
}

/// Strings that can be written without quotes (HJSON "quoteless" strings).
fn needs_quotes(s: &str) -> bool {
    if s.is_empty() || s.trim() != s || s.chars().any(|c| c.is_control()) {
        return true;
    }
    let first = s.chars().next().unwrap_or(' ');
    if matches!(first, '{' | '}' | '[' | ']' | ',' | ':' | '"' | '\'' | '#' | '`') {
        return true;
    }
    if s.starts_with("//") || s.starts_with("/*") {
        return true;
    }
    let end = s
        .find(|c: char| c.is_whitespace() || matches!(c, ',' | '}' | ']' | '#'))
        .unwrap_or(s.len());
    let tok = &s[..end];
    matches!(tok, "true" | "false" | "null") || is_json_number(tok)
}

fn write_str(s: &str, out: &mut String) {
    if needs_quotes(s) {
        write_quoted(s, out);
    } else {
        out.push_str(s);
    }
}

fn write_key(k: &str, out: &mut String) {
    if key_is_plain(k) {
        out.push_str(k);
    } else {
        write_quoted(k, out);
    }
}

fn write_value(v: &Value, ind: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::Str(s) => write_str(s, out),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
            } else {
                out.push_str("[\n");
                for x in a {
                    pad(out, ind + 1);
                    write_value(x, ind + 1, out);
                    out.push('\n');
                }
                pad(out, ind);
                out.push(']');
            }
        }
        Value::Object(o) => {
            if o.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{\n");
                for (k, x) in o {
                    pad(out, ind + 1);
                    write_key(k, out);
                    out.push_str(": ");
                    write_value(x, ind + 1, out);
                    out.push('\n');
                }
                pad(out, ind);
                out.push('}');
            }
        }
    }
}

/// Strict JSON output (quoted keys, commas) for files other tools read, e.g. Rojo projects.
pub fn to_json_string(v: &Value) -> String {
    let mut s = String::new();
    write_json(v, 0, &mut s);
    s.push('\n');
    s
}

fn write_json(v: &Value, ind: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::Str(s) => write_quoted(s, out),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
            } else if a.len() <= 16 && a.iter().all(|x| matches!(x, Value::Number(_) | Value::Bool(_) | Value::Null)) {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_json(x, ind, out);
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for (i, x) in a.iter().enumerate() {
                    pad(out, ind + 1);
                    write_json(x, ind + 1, out);
                    out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
                }
                pad(out, ind);
                out.push(']');
            }
        }
        Value::Object(o) => {
            if o.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{\n");
                for (i, (k, x)) in o.iter().enumerate() {
                    pad(out, ind + 1);
                    write_quoted(k, out);
                    out.push_str(": ");
                    write_json(x, ind + 1, out);
                    out.push_str(if i + 1 < o.len() { ",\n" } else { "\n" });
                }
                pad(out, ind);
                out.push('}');
            }
        }
    }
}

/// Like `to_string` but starts with a `#` comment line.
pub fn to_string_with_header(v: &Value, header: &str) -> String {
    format!("# {header}\n{}", to_string(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_style_metadata() {
        let v = parse(
            "{\n  \"$className\": \"Part\"\n  \"Anchored\": true\n  \"Size\": {\n    \"X\": 512\n    \"Y\": 1\n  }\n  \"Tags\": []\n}",
        )
        .unwrap();
        assert_eq!(v.get("$className").and_then(|x| x.as_str()), Some("Part"));
        assert_eq!(v.get("Anchored"), Some(&Value::Bool(true)));
        assert_eq!(v.get("Size").and_then(|s| s.get("X")), Some(&Value::Number("512".into())));
    }

    #[test]
    fn supports_comments_and_quoteless() {
        let v = parse("{\n # c\n a: hello world\n b: 3, // x\n}").unwrap();
        assert_eq!(v.get("a").and_then(|x| x.as_str()), Some("hello world"));
        assert_eq!(v.get("b"), Some(&Value::Number("3".into())));
    }

    #[test]
    fn writes_real_hjson() {
        let v = Value::Object(vec![
            ("$className".into(), Value::Str("Part".into())),
            ("Anchored".into(), Value::Bool(true)),
            ("note".into(), Value::Str("hello world".into())),
        ]);
        let text = to_string(&v);
        assert!(text.contains("$className: Part"));
        assert!(!text.contains("\"$className\""));
        assert!(text.contains("note: hello world"));
    }

    #[test]
    fn tricky_strings_round_trip() {
        for s in ["true, x", "3 apples", "  pad", "", "null", "42", "a:b", "#x", "{x}", "back\\slash", "say \"hi\"", "multi\nline"] {
            let v = Value::Object(vec![("k".into(), Value::Str(s.to_string()))]);
            assert_eq!(parse(&to_string(&v)).unwrap(), v, "string {s:?}");
        }
    }

    #[test]
    fn writer_round_trips() {
        let v = Value::Object(vec![
            ("a".into(), Value::Str("q\"\n\u{1}".into())),
            ("b".into(), Value::Array(vec![Value::Number("1.5e3".into()), Value::Null])),
            ("c".into(), Value::Object(vec![])),
        ]);
        assert_eq!(parse(&to_string(&v)).unwrap(), v);
    }
}
