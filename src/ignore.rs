//! .stewdioignore support (gitignore-like subset):
//! comments, `*`, `?`, `**`, trailing `/` (directories only), leading `/` (anchored), `!` negation.
use std::fs;
use std::path::Path;

struct Rule {
    pat: Vec<char>,
    dir_only: bool,
    with_path: bool,
    negate: bool,
}

pub struct IgnoreSet {
    rules: Vec<Rule>,
}

impl IgnoreSet {
    pub fn load(root: &Path) -> Self {
        match fs::read_to_string(root.join(".stewdioignore")) {
            Ok(t) => Self::parse(&t),
            Err(_) => IgnoreSet { rules: Vec::new() },
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for line in text.lines() {
            let mut l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let mut negate = false;
            if let Some(s) = l.strip_prefix('!') {
                negate = true;
                l = s;
            }
            let mut dir_only = false;
            let mut with_path = false;
            if let Some(s) = l.strip_suffix('/') {
                dir_only = true;
                l = s;
            }
            if let Some(s) = l.strip_prefix('/') {
                with_path = true;
                l = s;
            }
            if l.contains('/') {
                with_path = true;
            }
            if l.is_empty() {
                continue;
            }
            rules.push(Rule { pat: l.chars().collect(), dir_only, with_path, negate });
        }
        IgnoreSet { rules }
    }

    /// `rel` uses '/' separators and is relative to the project root. The last matching rule wins.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        let mut ignored = false;
        for r in &self.rules {
            if r.dir_only && !is_dir {
                continue;
            }
            let target: Vec<char> = if r.with_path { rel.chars().collect() } else { base.chars().collect() };
            if glob(&r.pat, &target) {
                ignored = !r.negate;
            }
        }
        ignored
    }
}

fn glob(p: &[char], t: &[char]) -> bool {
    match p.first().copied() {
        None => t.is_empty(),
        Some('*') => {
            let double = p.get(1) == Some(&'*');
            let rest = if double { &p[2..] } else { &p[1..] };
            for k in 0..=t.len() {
                if glob(rest, &t[k..]) {
                    return true;
                }
                if k < t.len() && !double && t[k] == '/' {
                    break;
                }
            }
            false
        }
        Some('?') => !t.is_empty() && t[0] != '/' && glob(&p[1..], &t[1..]),
        Some(c) => !t.is_empty() && t[0] == c && glob(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rules() {
        let s = IgnoreSet::parse("# c\nbuild/\n*.tmp\nPrivateAssets/\n");
        assert!(s.is_ignored("build", true));
        assert!(!s.is_ignored("build", false));
        assert!(s.is_ignored("Workspace/a.tmp", false));
        assert!(s.is_ignored("ServerStorage/PrivateAssets", true));
        assert!(!s.is_ignored("Workspace/Part", true));
    }

    #[test]
    fn negation_and_double_star() {
        let s = IgnoreSet::parse("*.bak\n!keep.bak\n**/Scratch\n");
        assert!(s.is_ignored("a/x.bak", false));
        assert!(!s.is_ignored("a/keep.bak", false));
        assert!(s.is_ignored("Workspace/deep/Scratch", true));
    }
}
