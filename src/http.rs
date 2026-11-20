//! Minimal HTTP client that shells out to `curl`.
//! This keeps Stewdio free of TLS dependencies (important for the many cross-compiled targets)
//! and keeps secrets out of the process argument list (headers go through curl's stdin config).
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "")
}

fn run_curl(method: &str, cfg: String) -> Result<Resp, String> {
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--compressed",
            "--max-time",
            "900",
            "-X",
            method,
            "-w",
            "\n__STEWDIO_STATUS__%{http_code}",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "`curl` was not found on PATH; install curl to use network features".to_string()
            } else {
                format!("failed to run curl: {e}")
            }
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(cfg.as_bytes()).map_err(|e| format!("failed to talk to curl: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        return Err(format!("network request failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let marker: &[u8] = b"\n__STEWDIO_STATUS__";
    let pos = out
        .stdout
        .windows(marker.len())
        .rposition(|w| w == marker)
        .ok_or_else(|| "unexpected curl output".to_string())?;
    let status: u16 = String::from_utf8_lossy(&out.stdout[pos + marker.len()..])
        .trim()
        .parse()
        .map_err(|_| "could not read HTTP status".to_string())?;
    Ok(Resp { status, body: out.stdout[..pos].to_vec() })
}

fn header_lines(headers: &[(&str, String)]) -> String {
    let mut cfg = String::new();
    for (k, v) in headers {
        cfg.push_str(&format!("header = \"{}: {}\"\n", esc(k), esc(v)));
    }
    cfg
}

pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body_file: Option<&Path>,
    body_text: Option<&str>,
) -> Result<Resp, String> {
    let mut cfg = header_lines(headers);
    if let Some(p) = body_file {
        cfg.push_str(&format!("data-binary = \"@{}\"\n", esc(&p.to_string_lossy())));
    }
    if let Some(t) = body_text {
        cfg.push_str(&format!("data = \"{}\"\n", esc(t)));
    }
    cfg.push_str(&format!("url = \"{}\"\n", esc(url)));
    run_curl(method, cfg)
}

/// multipart/form-data request; each entry of `forms` is a curl `-F` argument such as
/// `name=value`, `name=@"file";type=image/png` or `name=<"file";type=application/json`.
pub fn request_form(method: &str, url: &str, headers: &[(&str, String)], forms: &[String]) -> Result<Resp, String> {
    let mut cfg = header_lines(headers);
    for f in forms {
        cfg.push_str(&format!("form = \"{}\"\n", esc(f)));
    }
    cfg.push_str(&format!("url = \"{}\"\n", esc(url)));
    run_curl(method, cfg)
}
