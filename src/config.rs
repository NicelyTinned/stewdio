//! Global Stewdio configuration (`stewdio config`).
use clap::{Args, Subcommand};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::error::*;
use crate::util;

pub const DEFAULT_API_URL: &str =
    "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/Full-API-Dump.json";

/// key, allowed values, default
pub const KEYS: &[(&str, &str, &str)] = &[
    ("compiler.format", "binary|xml", "binary"),
    ("compiler.output_dir", "path", "build"),
    ("compiler.compression", "automatic|enabled|disabled", "automatic"),
    ("compiler.autofill", "off|normalize|defaults", "normalize"),
    ("compiler.preserve_unknown_properties", "true|false", "true"),
    ("templates.name", "baseplate|empty", "baseplate"),
    ("api.url", "url", DEFAULT_API_URL),
    ("api.max_age_days", "days", "30"),
];

pub struct Global {
    table: toml::Table,
}

impl Global {
    pub fn path() -> PathBuf {
        util::config_dir().join("config.toml")
    }

    pub fn load() -> Global {
        let table = fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str::<toml::Table>(&s).ok())
            .unwrap_or_default();
        Global { table }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let (sec, name) = key.split_once('.')?;
        let v = self.table.get(sec)?.as_table()?.get(name)?;
        Some(match v {
            toml::Value::String(s) => s.clone(),
            toml::Value::Boolean(b) => b.to_string(),
            other => other.to_string(),
        })
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let spec = KEYS.iter().find(|(k, _, _)| *k == key);
        let (_, allowed, _) = match spec {
            Some(s) => s,
            None => {
                let hint = util::suggest(key, KEYS.iter().map(|(k, _, _)| *k))
                    .map(|s| format!(" (did you mean '{s}'?)"))
                    .unwrap_or_default();
                return Err(format!("unknown setting '{key}'{hint}"));
            }
        };
        if allowed.contains('|') && !allowed.split('|').any(|a| a == value) {
            return Err(format!("invalid value '{value}' for {key} (allowed: {allowed})"));
        }
        let (sec, name) = match key.split_once('.') {
            Some(x) => x,
            None => return Err("invalid key".to_string()),
        };
        let val = if *allowed == "true|false" {
            toml::Value::Boolean(value == "true")
        } else {
            toml::Value::String(value.to_string())
        };
        let entry = self
            .table
            .entry(sec.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(t) = entry.as_table_mut() {
            t.insert(name.to_string(), val);
        }
        Ok(())
    }

    pub fn save(&self) -> io::Result<()> {
        let p = Self::path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(&self.table).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        fs::write(p, text)
    }
}

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub cmd: Option<ConfigCmd>,
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Print the value of one setting
    Get { key: String },
    /// Change one setting
    Set { key: String, value: String },
    /// List all settings
    List,
    /// Delete the global configuration file
    Reset,
}

fn print_list(g: &Global) {
    for (k, allowed, def) in KEYS {
        match g.get(k) {
            Some(v) => println!("{k} = {v}    ({allowed})"),
            None => println!("{k} = {def}    ({allowed}, default)"),
        }
    }
}

pub fn run(a: ConfigArgs) -> CliResult<()> {
    let mut g = Global::load();
    match a.cmd {
        Some(ConfigCmd::Get { key }) => match g.get(&key) {
            Some(v) => println!("{v}"),
            None => match KEYS.iter().find(|(k, _, _)| *k == key) {
                Some((_, _, d)) => println!("{d}"),
                None => return Err(CliError::new(ARGS, format!("unknown setting '{key}'"))),
            },
        },
        Some(ConfigCmd::Set { key, value }) => {
            g.set(&key, &value).map_err(|e| CliError::new(ARGS, e))?;
            g.save()?;
            println!("{key} = {value}");
        }
        Some(ConfigCmd::List) => print_list(&g),
        Some(ConfigCmd::Reset) => {
            let p = Global::path();
            if p.exists() {
                if !util::confirm("Delete the global Stewdio configuration?") {
                    return Err(CliError::new(GENERAL, "Aborted"));
                }
                fs::remove_file(p)?;
            }
            println!("Configuration reset");
        }
        None => {
            println!("Interactive configuration (press Enter to keep the current value)\n");
            for (k, allowed, def) in KEYS {
                let cur = g.get(k).unwrap_or_else(|| def.to_string());
                print!("{k} [{cur}] ({allowed}): ");
                let _ = io::stdout().flush();
                let mut line = String::new();
                if io::stdin().lock().read_line(&mut line)? == 0 {
                    break;
                }
                let v = line.trim();
                if v.is_empty() {
                    continue;
                }
                if let Err(e) = g.set(k, v) {
                    crate::ui::warning(&e);
                }
            }
            g.save()?;
            println!("\nSaved to {}", Global::path().display());
        }
    }
    Ok(())
}
