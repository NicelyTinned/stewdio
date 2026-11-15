//! Minimal XML reader/writer, sufficient for Roblox .rbxlx files.

#[derive(Debug, Clone)]
pub enum Node {
    Element(Element),
    Text(String),
    CData(String),
}

#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Element { name: name.to_string(), attrs: Vec::new(), children: Vec::new() }
    }

    pub fn leaf(name: &str, text: &str) -> Self {
        let mut e = Self::new(name);
        e.children.push(Node::Text(text.to_string()));
        e
    }

    pub fn with_attr(mut self, k: &str, v: &str) -> Self {
        self.attrs.push((k.to_string(), v.to_string()));
        self
    }

    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(key, _)| key.as_str() == k).map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> + '_ {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    pub fn has_elements(&self) -> bool {
        self.children.iter().any(|n| matches!(n, Node::Element(_)))
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.children {
            match n {
                Node::Text(t) | Node::CData(t) => s.push_str(t),
                _ => {}
            }
        }
        s
    }

    pub fn write(&self, out: &mut String, depth: usize) {
        let pad = "\t".repeat(depth);
        out.push_str(&pad);
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(&escape_attr(v));
            out.push('"');
        }
        out.push('>');
        if !self.has_elements() {
            for n in &self.children {
                match n {
                    Node::Text(t) => out.push_str(&escape_text(t)),
                    Node::CData(t) => {
                        out.push_str("<![CDATA[");
                        out.push_str(&t.replace("]]>", "]]]]><![CDATA[>"));
                        out.push_str("]]>");
                    }
                    Node::Element(_) => {}
                }
            }
        } else {
            out.push('\n');
            for n in &self.children {
                if let Node::Element(e) = n {
                    e.write(out, depth + 1);
                }
            }
            out.push_str(&pad);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(">\n");
    }

    pub fn to_document(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0);
        s
    }
}

pub fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find(';') {
            let ent = &after[..j];
            let rep = match ent {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = rep {
                out.push(c);
                rest = &after[j + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn starts(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }

    fn err(&self, m: &str) -> String {
        let before = &self.s[..self.pos.min(self.s.len())];
        let line = before.matches('\n').count() + 1;
        format!("XML error: {m} (line {line})")
    }

    fn skip_ws(&mut self) {
        let r = self.rest();
        let t = r.trim_start();
        self.pos += r.len() - t.len();
    }

    fn skip_until(&mut self, end: &str) -> Result<(), String> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => Err(self.err(&format!("unterminated construct, expected '{end}'"))),
        }
    }

    fn name(&mut self) -> Result<String, String> {
        let r = self.rest();
        let end = r
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>' || c == '=' || c == '<')
            .unwrap_or(r.len());
        if end == 0 {
            return Err(self.err("expected a name"));
        }
        let n = r[..end].to_string();
        self.pos += end;
        Ok(n)
    }

    fn prolog(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            if self.starts("<?") {
                self.skip_until("?>")?;
            } else if self.starts("<!--") {
                self.skip_until("-->")?;
            } else if self.starts("<!") {
                self.skip_until(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn element(&mut self) -> Result<Element, String> {
        if !self.starts("<") {
            return Err(self.err("expected '<'"));
        }
        self.pos += 1;
        let name = self.name()?;
        let mut el = Element::new(&name);
        loop {
            self.skip_ws();
            if self.starts("/>") {
                self.pos += 2;
                return Ok(el);
            }
            if self.starts(">") {
                self.pos += 1;
                break;
            }
            if self.rest().is_empty() {
                return Err(self.err("unexpected end of file inside a tag"));
            }
            let key = self.name()?;
            self.skip_ws();
            if !self.starts("=") {
                return Err(self.err("expected '=' after attribute name"));
            }
            self.pos += 1;
            self.skip_ws();
            let q = match self.rest().chars().next() {
                Some(c) if c == '"' || c == '\'' => c,
                _ => return Err(self.err("expected a quoted attribute value")),
            };
            self.pos += 1;
            let end = match self.rest().find(q) {
                Some(e) => e,
                None => return Err(self.err("unterminated attribute value")),
            };
            let raw = &self.s[self.pos..self.pos + end];
            el.attrs.push((key, decode_entities(raw)));
            self.pos += end + 1;
        }
        loop {
            if self.rest().is_empty() {
                return Err(self.err(&format!("unclosed element <{}>", el.name)));
            }
            if self.starts("</") {
                self.pos += 2;
                let close = self.name()?;
                self.skip_ws();
                if !self.starts(">") {
                    return Err(self.err("expected '>' in closing tag"));
                }
                self.pos += 1;
                if close != el.name {
                    return Err(self.err(&format!("mismatched closing tag </{close}> for <{}>", el.name)));
                }
                return Ok(el);
            } else if self.starts("<!--") {
                self.pos += 4;
                self.skip_until("-->")?;
            } else if self.starts("<![CDATA[") {
                self.pos += 9;
                let end = match self.rest().find("]]>") {
                    Some(e) => e,
                    None => return Err(self.err("unterminated CDATA section")),
                };
                el.children.push(Node::CData(self.s[self.pos..self.pos + end].to_string()));
                self.pos += end + 3;
            } else if self.starts("<?") {
                self.pos += 2;
                self.skip_until("?>")?;
            } else if self.starts("<") {
                let child = self.element()?;
                el.children.push(Node::Element(child));
            } else {
                let r = self.rest();
                let end = r.find('<').unwrap_or(r.len());
                el.children.push(Node::Text(decode_entities(&r[..end])));
                self.pos += end;
            }
        }
    }
}

pub fn parse(input: &str) -> Result<Element, String> {
    let input = input.trim_start_matches('\u{feff}');
    let mut p = Parser { s: input, pos: 0 };
    p.prolog()?;
    p.element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_document() {
        let e = parse("<?xml version=\"1.0\"?><a x=\"1&amp;2\"><b>t&lt;</b><![CDATA[c]]></a>").unwrap();
        assert_eq!(e.name, "a");
        assert_eq!(e.attr("x"), Some("1&2"));
        assert_eq!(e.elements().next().unwrap().text(), "t<");
    }

    #[test]
    fn writer_round_trips() {
        let e = parse("<a><b k=\"v\">x &amp; y</b></a>").unwrap();
        let out = e.to_document();
        let again = parse(&out).unwrap();
        assert_eq!(again.elements().next().unwrap().text(), "x & y");
    }
}
