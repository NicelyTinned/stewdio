//! `stewdio auth`: manage Roblox Open Cloud API keys.
use clap::{Args, Subcommand};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};

use crate::error::*;
use crate::{cred, hjson, http, ui, util};

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub cmd: Option<AuthCmd>,
}

#[derive(Subcommand)]
pub enum AuthCmd {
    /// Configure an API key
    Login {
        /// Profile name (default: "default")
        #[arg(long)]
        profile: Option<String>,
        /// Read the key from standard input instead of prompting
        #[arg(long)]
        stdin: bool,
    },
    /// Remove stored credentials
    Logout {
        #[arg(long)]
        profile: Option<String>,
    },
    /// Display authentication status
    Status {
        /// Also ask Roblox whether the key is valid
        #[arg(long)]
        check: bool,
    },
    /// List configured authentication profiles
    List,
    /// Select an authentication profile
    Use { profile: String },
}

// ---- profile bookkeeping (names only, never secrets)

pub struct Profiles {
    pub default: String,
    pub names: Vec<String>,
}

fn profiles_path() -> std::path::PathBuf {
    util::config_dir().join("profiles.toml")
}

impl Profiles {
    pub fn load() -> Profiles {
        let t = fs::read_to_string(profiles_path())
            .ok()
            .and_then(|s| toml::from_str::<toml::Table>(&s).ok())
            .unwrap_or_default();
        let default = t.get("default").and_then(|v| v.as_str()).unwrap_or("default").to_string();
        let names: Vec<String> = t
            .get("profiles")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        Profiles { default, names }
    }

    fn save(&self) -> io::Result<()> {
        let mut t = toml::Table::new();
        t.insert("default".to_string(), toml::Value::String(self.default.clone()));
        t.insert(
            "profiles".to_string(),
            toml::Value::Array(self.names.iter().map(|n| toml::Value::String(n.clone())).collect()),
        );
        let p = profiles_path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, toml::to_string(&t).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?)
    }
}

/// The API key to use for `profile` (or the default profile). Environment variable STEWDIO_API_KEY wins.
pub fn current_key(profile: Option<&str>) -> Option<String> {
    let p = Profiles::load();
    let name = profile.map(|s| s.to_string()).unwrap_or(p.default);
    cred::load(&name)
}

fn prompt_secret(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    if cfg!(windows) {
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$s = Read-Host -AsSecureString; [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($s))",
            ])
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| e.to_string())?;
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let _ = Command::new("stty").arg("-echo").stdin(Stdio::inherit()).status();
    let mut line = String::new();
    let r = io::stdin().lock().read_line(&mut line);
    let _ = Command::new("stty").arg("echo").stdin(Stdio::inherit()).status();
    println!();
    r.map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

/// Ask Roblox about a key. Ok(Some(description)) when valid, Ok(None) when we could not tell.
fn introspect(key: &str) -> Result<Option<String>, CliError> {
    let body = format!("{{\"apiKey\":\"{}\"}}", key.replace('\\', "\\\\").replace('"', "\\\""));
    let resp = match http::request(
        "POST",
        "https://apis.roblox.com/api-keys/v1/introspect",
        &[("Content-Type", "application/json".to_string())],
        None,
        Some(&body),
    ) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if resp.status == 401 || resp.status == 400 {
        return Err(CliError::new(AUTH, "Roblox authentication failed: the API key was rejected"));
    }
    if resp.status != 200 {
        return Ok(None);
    }
    let v = match hjson::parse(&String::from_utf8_lossy(&resp.body)) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if v.get("enabled").and_then(|x| x.as_bool()) == Some(false) {
        return Err(CliError::new(AUTH, "Roblox authentication failed: this API key is disabled"));
    }
    if v.get("expired").and_then(|x| x.as_bool()) == Some(true) {
        return Err(CliError::new(AUTH, "Roblox authentication failed: this API key has expired"));
    }
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("(unnamed key)");
    let mut scopes = Vec::new();
    if let Some(a) = v.get("scopes").and_then(|x| x.as_array()) {
        for s in a {
            if let Some(n) = s.get("name").and_then(|x| x.as_str()) {
                scopes.push(n.to_string());
            }
        }
    }
    Ok(Some(format!("key \"{}\", scopes: {}", name, if scopes.is_empty() { "none listed".to_string() } else { scopes.join(", ") })))
}

pub fn run(a: AuthArgs) -> CliResult<()> {
    match a.cmd.unwrap_or(AuthCmd::Login { profile: None, stdin: false }) {
        AuthCmd::Login { profile, stdin } => {
            let name = profile.unwrap_or_else(|| "default".to_string());
            if !cred::valid_profile(&name) {
                return Err(CliError::new(ARGS, "Profile names may only contain letters, digits, '-' and '_'"));
            }
            let key = if stdin {
                let mut s = String::new();
                io::stdin().read_to_string(&mut s)?;
                s.trim().to_string()
            } else {
                prompt_secret("Roblox Open Cloud API key: ").map_err(|e| CliError::new(AUTH, e))?
            };
            if key.is_empty() {
                return Err(CliError::new(AUTH, "No API key entered"));
            }
            match introspect(&key)? {
                Some(info) => ui::step(&format!("Validated {info}")),
                None => ui::warning("Could not validate the key online; storing it anyway"),
            }
            let where_ = cred::store(&name, &key).map_err(|e| CliError::new(AUTH, e))?;
            let mut p = Profiles::load();
            if !p.names.contains(&name) {
                p.names.push(name.clone());
            }
            if p.names.len() == 1 {
                p.default = name.clone();
            }
            p.save()?;
            println!("Stored profile '{name}' in {where_}");
            if p.default != name {
                println!("Run `stewdio auth use {name}` to make it the default profile");
            }
        }
        AuthCmd::Logout { profile } => {
            let mut p = Profiles::load();
            let name = profile.unwrap_or_else(|| p.default.clone());
            cred::delete(&name);
            p.names.retain(|n| *n != name);
            if p.default == name {
                p.default = p.names.first().cloned().unwrap_or_else(|| "default".to_string());
            }
            p.save()?;
            println!("Removed credentials for profile '{name}'");
        }
        AuthCmd::Status { check } => {
            let p = Profiles::load();
            println!("Profile: {}", p.default);
            if std::env::var("STEWDIO_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false) {
                println!("Source:  STEWDIO_API_KEY environment variable");
            }
            match cred::load(&p.default) {
                Some(key) => {
                    println!("Status:  authenticated (an API key is configured)");
                    if check {
                        match introspect(&key)? {
                            Some(info) => println!("Valid:   {info}"),
                            None => println!("Valid:   could not be verified online"),
                        }
                    }
                }
                None => {
                    println!("Status:  not authenticated");
                    ui::hint("run `stewdio auth login`");
                }
            }
        }
        AuthCmd::List => {
            let p = Profiles::load();
            if p.names.is_empty() {
                println!("No profiles configured");
            }
            for n in &p.names {
                println!("{} {}", if *n == p.default { "*" } else { " " }, n);
            }
        }
        AuthCmd::Use { profile } => {
            let mut p = Profiles::load();
            if !p.names.contains(&profile) {
                return Err(CliError::new(ARGS, format!("No profile named '{profile}' (see `stewdio auth list`)")));
            }
            p.default = profile.clone();
            p.save()?;
            println!("Now using profile '{profile}'");
        }
    }
    Ok(())
}
