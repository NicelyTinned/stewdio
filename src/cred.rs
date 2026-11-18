//! Credential storage. Prefers the operating system's secret store; falls back to a
//! user-only (0600) file in the Stewdio config directory when none is available.
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::util;

pub fn valid_profile(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn run(prog: &str, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Result<(bool, String), String> {
    let mut c = Command::new(prog);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut ch = c.spawn().map_err(|e| format!("{prog}: {e}"))?;
    if let Some(s) = stdin {
        if let Some(mut i) = ch.stdin.take() {
            let _ = i.write_all(s.as_bytes());
        }
    }
    let out = ch.wait_with_output().map_err(|e| e.to_string())?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

fn file_path(profile: &str, ext: &str) -> PathBuf {
    util::config_dir().join(format!("cred-{profile}.{ext}"))
}

fn write_secret_file(path: &PathBuf, content: &str) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(content.as_bytes())
}

const PS_ENC: &str = "$s = ConvertTo-SecureString -String $env:STEWDIO_SECRET -AsPlainText -Force; ConvertFrom-SecureString $s";
const PS_DEC: &str = "$s = ConvertTo-SecureString $env:STEWDIO_ENC; [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($s))";

fn os_store(profile: &str, key: &str) -> Result<&'static str, String> {
    if cfg!(target_os = "macos") {
        let (ok, _) = run(
            "security",
            &["add-generic-password", "-U", "-s", "stewdio", "-a", profile, "-w", key],
            None,
            &[],
        )?;
        if ok {
            return Ok("macOS Keychain");
        }
        Err("keychain write failed".to_string())
    } else if cfg!(windows) {
        let (ok, enc) = run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", PS_ENC],
            None,
            &[("STEWDIO_SECRET", key)],
        )?;
        if !ok || enc.is_empty() {
            return Err("DPAPI encryption failed".to_string());
        }
        write_secret_file(&file_path(profile, "dpapi"), &enc).map_err(|e| e.to_string())?;
        Ok("Windows DPAPI (user-encrypted file)")
    } else {
        let (ok, _) = run(
            "secret-tool",
            &["store", "--label=Stewdio API key", "service", "stewdio", "account", profile],
            Some(key),
            &[],
        )?;
        if ok {
            return Ok("system keyring (secret-tool)");
        }
        Err("secret-tool failed".to_string())
    }
}

fn os_load(profile: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        let (ok, v) = run("security", &["find-generic-password", "-s", "stewdio", "-a", profile, "-w"], None, &[]).ok()?;
        if ok && !v.is_empty() { Some(v) } else { None }
    } else if cfg!(windows) {
        let enc = fs::read_to_string(file_path(profile, "dpapi")).ok()?;
        let (ok, v) = run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", PS_DEC],
            None,
            &[("STEWDIO_ENC", enc.trim())],
        )
        .ok()?;
        if ok && !v.is_empty() { Some(v) } else { None }
    } else {
        let (ok, v) = run("secret-tool", &["lookup", "service", "stewdio", "account", profile], None, &[]).ok()?;
        if ok && !v.is_empty() { Some(v) } else { None }
    }
}

fn os_delete(profile: &str) {
    if cfg!(target_os = "macos") {
        let _ = run("security", &["delete-generic-password", "-s", "stewdio", "-a", profile], None, &[]);
    } else if cfg!(windows) {
        let _ = fs::remove_file(file_path(profile, "dpapi"));
    } else {
        let _ = run("secret-tool", &["clear", "service", "stewdio", "account", profile], None, &[]);
    }
}

/// Returns a description of where the key was stored.
pub fn store(profile: &str, key: &str) -> Result<String, String> {
    match os_store(profile, key) {
        Ok(name) => {
            let _ = fs::remove_file(file_path(profile, "key"));
            Ok(name.to_string())
        }
        Err(_) => {
            write_secret_file(&file_path(profile, "key"), key).map_err(|e| format!("cannot store credentials: {e}"))?;
            Ok(format!(
                "protected file {} (no OS credential store available)",
                file_path(profile, "key").display()
            ))
        }
    }
}

pub fn load(profile: &str) -> Option<String> {
    if let Ok(k) = std::env::var("STEWDIO_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    os_load(profile).or_else(|| fs::read_to_string(file_path(profile, "key")).ok().map(|s| s.trim().to_string()))
}

pub fn delete(profile: &str) {
    os_delete(profile);
    let _ = fs::remove_file(file_path(profile, "key"));
}
