//! Luau syntax highlighting and lightweight diagnostics (block balance).

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tok {
    Plain,
    Keyword,
    Builtin,
    Str,
    Number,
    Comment,
    Op,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Carry {
    None,
    Long { eq: usize, comment: bool },
}

const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

const BUILTINS: &[&str] = &[
    "print", "warn", "error", "assert", "pairs", "ipairs", "next", "select", "tostring", "tonumber",
    "type", "typeof", "require", "pcall", "xpcall", "spawn", "delay", "wait", "task", "game",
    "workspace", "script", "Instance", "Vector3", "Vector2", "CFrame", "Color3", "UDim2", "UDim",
    "Enum", "math", "string", "table", "os", "coroutine", "bit32", "utf8", "debug", "setmetatable",
    "getmetatable", "rawget", "rawset", "rawequal", "unpack", "shared", "tick", "time", "Random",
    "TweenInfo", "Rect", "Ray", "Region3", "BrickColor", "NumberSequence", "ColorSequence",
    "NumberRange", "Font", "buffer", "self",
];

fn find_long_close(chars: &[char], from: usize, eq: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == ']' {
            let mut j = i + 1;
            let mut c = 0;
            while j < chars.len() && chars[j] == '=' {
                c += 1;
                j += 1;
            }
            if c == eq && j < chars.len() && chars[j] == ']' {
                return Some(j + 1);
            }
        }
        i += 1;
    }
    None
}

/// At a `[`: returns (number of `=`, index just after the opening bracket).
fn long_open(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if chars.get(i) != Some(&'[') {
        return None;
    }
    let mut j = i + 1;
    let mut eq = 0;
    while chars.get(j) == Some(&'=') {
        eq += 1;
        j += 1;
    }
    if chars.get(j) == Some(&'[') {
        Some((eq, j + 1))
    } else {
        None
    }
}

fn mark(out: &mut [Tok], from: usize, to: usize, t: Tok) {
    let end = to.min(out.len());
    for k in from..end {
        out[k] = t;
    }
}

/// Classify every character of one line; `carry` is the multi-line state at its start.
pub fn lex_line(chars: &[char], carry: Carry) -> (Vec<Tok>, Carry) {
    let n = chars.len();
    let mut out = vec![Tok::Plain; n];
    let mut i = 0;
    if let Carry::Long { eq, comment } = carry {
        let t = if comment { Tok::Comment } else { Tok::Str };
        match find_long_close(chars, 0, eq) {
            Some(end) => {
                mark(&mut out, 0, end, t);
                i = end;
            }
            None => {
                mark(&mut out, 0, n, t);
                return (out, carry);
            }
        }
    }
    while i < n {
        let c = chars[i];
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            if let Some((eq, after)) = long_open(chars, i + 2) {
                match find_long_close(chars, after, eq) {
                    Some(end) => {
                        mark(&mut out, i, end, Tok::Comment);
                        i = end;
                        continue;
                    }
                    None => {
                        mark(&mut out, i, n, Tok::Comment);
                        return (out, Carry::Long { eq, comment: true });
                    }
                }
            }
            mark(&mut out, i, n, Tok::Comment);
            break;
        }
        if c == '"' || c == '\'' || c == '`' {
            let st = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(n);
            mark(&mut out, st, end, Tok::Str);
            i = end;
            continue;
        }
        if c == '[' {
            if let Some((eq, after)) = long_open(chars, i) {
                match find_long_close(chars, after, eq) {
                    Some(end) => {
                        mark(&mut out, i, end, Tok::Str);
                        i = end;
                        continue;
                    }
                    None => {
                        mark(&mut out, i, n, Tok::Str);
                        return (out, Carry::Long { eq, comment: false });
                    }
                }
            }
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).map(|d| d.is_ascii_digit()).unwrap_or(false)) {
            let st = i;
            i += 1;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            mark(&mut out, st, i, Tok::Number);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[st..i].iter().collect();
            let t = if KEYWORDS.contains(&w.as_str()) {
                Tok::Keyword
            } else if BUILTINS.contains(&w.as_str()) {
                Tok::Builtin
            } else {
                Tok::Plain
            };
            mark(&mut out, st, i, t);
            continue;
        }
        if "+-*/%^#=~<>".contains(c) {
            out[i] = Tok::Op;
        }
        i += 1;
    }
    (out, Carry::None)
}

pub struct Diag {
    pub line: usize, // 0-based
    pub msg: String,
}

const EXPR_PREV: &[&str] = &[
    "=", "(", ",", "return", "{", "[", "+", "-", "*", "/", "%", "^", "and", "or", "not", "<", ">", "~", ".", ":",
];

/// Very small structural check: unbalanced `function/if/do/repeat` blocks.
pub fn diagnostics(lines: &[String]) -> Vec<Diag> {
    let mut diags: Vec<Diag> = Vec::new();
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    let mut carry = Carry::None;
    let mut prev = String::new();
    for (ln, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let (toks, next) = lex_line(&chars, carry);
        carry = next;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() || toks[i] == Tok::Comment {
                i += 1;
                continue;
            }
            if c.is_alphabetic() || c == '_' {
                let st = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let w: String = chars[st..i].iter().collect();
                if toks[st] == Tok::Keyword {
                    match w.as_str() {
                        "function" => stack.push(("function", ln)),
                        "do" => stack.push(("do", ln)),
                        "repeat" => stack.push(("repeat", ln)),
                        "if" => {
                            if !EXPR_PREV.contains(&prev.as_str()) {
                                stack.push(("if", ln));
                            }
                        }
                        "end" => {
                            let top = stack.last().map(|(k, _)| *k);
                            match top {
                                Some(k) if k != "repeat" => {
                                    stack.pop();
                                }
                                _ => diags.push(Diag { line: ln, msg: "unexpected 'end'".to_string() }),
                            }
                        }
                        "until" => {
                            let top = stack.last().map(|(k, _)| *k);
                            match top {
                                Some("repeat") => {
                                    stack.pop();
                                }
                                _ => diags.push(Diag { line: ln, msg: "unexpected 'until'".to_string() }),
                            }
                        }
                        _ => {}
                    }
                    prev = w;
                } else {
                    prev = "<id>".to_string();
                }
                continue;
            }
            if toks[i] == Tok::Str || toks[i] == Tok::Number {
                let t = toks[i];
                while i < chars.len() && toks[i] == t {
                    i += 1;
                }
                prev = "<lit>".to_string();
                continue;
            }
            prev = c.to_string();
            i += 1;
        }
    }
    for (k, l) in stack.iter().take(3) {
        diags.push(Diag { line: *l, msg: format!("unclosed '{k}'") });
    }
    diags.sort_by_key(|d| d.line);
    diags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn highlights_basics() {
        let chars: Vec<char> = "local x = 1 -- hi".chars().collect();
        let (t, _) = lex_line(&chars, Carry::None);
        assert_eq!(t[0], Tok::Keyword);
        assert_eq!(t[10], Tok::Number);
        assert_eq!(t[12], Tok::Comment);
    }

    #[test]
    fn long_comment_carries() {
        let (_, c) = lex_line(&"--[[ start".chars().collect::<Vec<_>>(), Carry::None);
        assert_eq!(c, Carry::Long { eq: 0, comment: true });
        let (_, c2) = lex_line(&"end ]] x".chars().collect::<Vec<_>>(), c);
        assert_eq!(c2, Carry::None);
    }

    #[test]
    fn balanced_code_is_clean() {
        let d = diagnostics(&lines("local function f(a)\n  if a then\n    for i = 1, 3 do print(i) end\n  end\n  local y = if a then 1 else 2\nend\nrepeat x() until true"));
        assert!(d.is_empty(), "{:?}", d.iter().map(|x| x.msg.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn finds_problems() {
        let d = diagnostics(&lines("if x then\n  print(1)\n"));
        assert_eq!(d.len(), 1);
        let d = diagnostics(&lines("print(1)\nend"));
        assert_eq!(d[0].msg, "unexpected 'end'");
    }
}
