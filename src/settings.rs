//! `stewdio settings`: edit Project.toml from the command line without losing comments or layout.
use clap::{Args, Subcommand};
use std::fs;
use std::io::{self, BufRead, Write};
use std::process::Command;
use toml_edit::{value, DocumentMut, Item, Table};

use crate::error::*;
use crate::project::Project;
use crate::ui;

/// key, allowed values / type
const KEYS: &[(&str, &str)] = &[
    ("project.name", "string"),
    ("project.version", "string"),
    ("project.type", "place|model"),
    ("roblox.universe_id", "int"),
    ("roblox.place_id", "int"),
    ("compiler.format", "binary|xml"),
    ("compiler.output", "path"),
    ("compiler.autofill", "off|normalize|defaults"),
    ("compiler.compression", "automatic|enabled|disabled"),
    ("compiler.preserve_unknown_properties", "bool"),
    ("decompiler.extract_scripts", "bool"),
    ("decompiler.extract_assets", "bool"),
    ("templates.name", "baseplate|empty"),
];

#[derive(Args)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub cmd: Option<SettingsCmd>,
}

#[derive(Subcommand)]
pub enum SettingsCmd {
    /// Show every setting in Project.toml
    List,
    /// Print one setting
    Get { key: String },
    /// Change one setting (e.g. `settings set compiler.format xml`)
    Set { key: String, value: String },
    /// Remove a setting so the default applies again
    Unset { key: String },
    /// Open Project.toml in $VISUAL / $EDITOR
    Edit,
    /// Print the path of Project.toml
    Path,
}

fn spec(key: &str) -> CliResult<&'static (&'static str, &'static str)> {
    KEYS.iter().find(|(k, _)| *k == key).ok_or_else(|| {
        let hint = crate::util::suggest(key, KEYS.iter().map(|(k, _)| *k))
            .map(|s| format!(" (did you mean '{s}'?)"))
            .unwrap_or_default();
        CliError::new(ARGS, format!("Unknown setting '{key}'{hint}"))
    })
}

fn read_doc(project: &Project) -> CliResult<(String, DocumentMut)> {
    let text = fs::read_to_string(project.root.join("Project.toml"))?;
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| CliError::new(PROJECT, format!("Invalid Project.toml: {e}")))?;
    Ok((text, doc))
}

fn show(doc: &DocumentMut, key: &str) -> Option<String> {
    let (sec, name) = key.split_once('.')?;
    let item = doc.get(sec)?.get(name)?;
    let v = item.as_value()?;
    Some(match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string().trim().to_string(),
    })
}

/// Write `text`, check that the project still loads, and roll back if it does not.
fn commit(project: &Project, old: &str, new: String) -> CliResult<()> {
    let path = project.root.join("Project.toml");
    fs::write(&path, &new)?;
    if let Err(e) = Project::load(&project.root) {
        fs::write(&path, old)?;
        return Err(e);
    }
    Ok(())
}

fn set_value(project: &Project, key: &str, raw: &str) -> CliResult<()> {
    let (_, kind) = spec(key)?;
    let (sec, name) = key.split_once('.').ok_or_else(|| CliError::new(ARGS, "Invalid key"))?;
    let item: Item = match *kind {
        "int" => value(raw.parse::<i64>().map_err(|_| CliError::new(ARGS, format!("{key} expects a whole number")))?),
        "bool" => match raw {
            "true" => value(true),
            "false" => value(false),
            _ => return Err(CliError::new(ARGS, format!("{key} expects true or false"))),
        },
        "string" | "path" => value(raw),
        allowed => {
            if !allowed.split('|').any(|a| a == raw) {
                return Err(CliError::new(ARGS, format!("{key} must be one of: {allowed}")));
            }
            value(raw)
        }
    };
    let (old, mut doc) = read_doc(project)?;
    if !doc.contains_key(sec) {
        doc[sec] = Item::Table(Table::new());
    }
    doc[sec][name] = item;
    commit(project, &old, doc.to_string())
}

fn unset_value(project: &Project, key: &str) -> CliResult<()> {
    spec(key)?;
    let (sec, name) = key.split_once('.').ok_or_else(|| CliError::new(ARGS, "Invalid key"))?;
    if key == "project.name" || key == "project.type" {
        return Err(CliError::new(ARGS, format!("{key} is required and cannot be removed")));
    }
    let (old, mut doc) = read_doc(project)?;
    if let Some(t) = doc.get_mut(sec).and_then(|i| i.as_table_mut()) {
        t.remove(name);
    }
    commit(project, &old, doc.to_string())
}

fn list(project: &Project) -> CliResult<()> {
    let (_, doc) = read_doc(project)?;
    for (k, kind) in KEYS {
        match show(&doc, k) {
            Some(v) => println!("{k} = {v}    ({kind})"),
            None => println!("{k}    (not set; {kind})"),
        }
    }
    Ok(())
}

pub fn run(a: SettingsArgs) -> CliResult<()> {
    let project = Project::current()?;
    match a.cmd {
        Some(SettingsCmd::List) => list(&project)?,
        Some(SettingsCmd::Get { key }) => {
            spec(&key)?;
            let (_, doc) = read_doc(&project)?;
            match show(&doc, &key) {
                Some(v) => println!("{v}"),
                None => return Err(CliError::new(GENERAL, format!("{key} is not set"))),
            }
        }
        Some(SettingsCmd::Set { key, value }) => {
            set_value(&project, &key, &value)?;
            println!("{key} = {value}");
        }
        Some(SettingsCmd::Unset { key }) => {
            unset_value(&project, &key)?;
            println!("Removed {key}");
        }
        Some(SettingsCmd::Path) => println!("{}", project.root.join("Project.toml").display()),
        Some(SettingsCmd::Edit) => {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() });
            let path = project.root.join("Project.toml");
            let backup = fs::read_to_string(&path)?;
            let status = Command::new(&editor)
                .arg(&path)
                .status()
                .map_err(|e| CliError::new(GENERAL, format!("could not start '{editor}': {e} (set $EDITOR)")))?;
            if !status.success() {
                ui::warning("the editor exited with an error");
            }
            if let Err(e) = Project::load(&project.root) {
                ui::error(&e.message);
                if util_confirm("Project.toml is now invalid. Restore the previous version?") {
                    fs::write(&path, backup)?;
                    println!("Restored");
                }
                return Err(e);
            }
            println!("Project.toml is valid");
        }
        None => interactive(&project)?,
    }
    Ok(())
}

fn util_confirm(q: &str) -> bool {
    crate::util::confirm(q)
}

fn interactive(project: &Project) -> CliResult<()> {
    loop {
        println!();
        let (_, doc) = read_doc(project)?;
        for (i, (k, kind)) in KEYS.iter().enumerate() {
            let cur = show(&doc, k).unwrap_or_else(|| "(not set)".to_string());
            println!("{:>2}. {k} = {cur}    ({kind})", i + 1);
        }
        print!("\nNumber or key to change (blank to finish): ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            break;
        }
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        let key = match t.parse::<usize>() {
            Ok(n) if n >= 1 && n <= KEYS.len() => KEYS[n - 1].0.to_string(),
            _ => t.to_string(),
        };
        let (_, kind) = match spec(&key) {
            Ok(s) => s,
            Err(e) => {
                ui::error(&e.message);
                continue;
            }
        };
        print!("New value for {key} ({kind}, empty = unset): ");
        let _ = io::stdout().flush();
        let mut v = String::new();
        io::stdin().lock().read_line(&mut v)?;
        let v = v.trim();
        let res = if v.is_empty() { unset_value(project, &key) } else { set_value(project, &key, v) };
        match res {
            Ok(()) => ui::success("Saved"),
            Err(e) => ui::error(&e.message),
        }
    }
    Ok(())
}
