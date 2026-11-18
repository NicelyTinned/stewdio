//! Tiny terminal styling helpers (colors only when attached to a terminal).
use std::io::IsTerminal;

fn color_ok(stderr: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if cfg!(windows) && std::env::var_os("WT_SESSION").is_none() && std::env::var_os("TERM").is_none() {
        return false;
    }
    if stderr {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    }
}

pub fn paint(code: &str, text: &str, stderr: bool) -> String {
    if color_ok(stderr) {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn error(msg: &str) {
    eprintln!("{} {msg}", paint("1;31", "[Error]", true));
}

pub fn warning(msg: &str) {
    eprintln!("{} {msg}", paint("33", "[Warning]", true));
}

pub fn hint(msg: &str) {
    eprintln!("{} {msg}", paint("2", "hint:", true));
}

pub fn step(msg: &str) {
    println!("{} {msg}", paint("36", "[Stewdio]", false));
}

pub fn success(msg: &str) {
    println!("{}", paint("1;32", msg, false));
}
