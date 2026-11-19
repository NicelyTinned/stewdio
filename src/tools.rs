//! Small quality-of-life commands: doctor, clean, watch.
use clap::Args;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::commands::{self, CompileArgs};
use crate::error::*;
use crate::ignore::IgnoreSet;
use crate::project::Project;
use crate::{api, auth, cred, ui, util};

// ---------------------------------------------------------------- doctor

fn check(ok: bool, what: &str, detail: &str) {
    let mark = if ok { ui::paint("32", "ok  ", false) } else { ui::paint("31", "FAIL", false) };
    if detail.is_empty() {
        println!("[{mark}] {what}");
    } else {
        println!("[{mark}] {what}: {detail}");
    }
}

pub fn doctor() -> CliResult<()> {
    println!("Stewdio {}", env!("CARGO_PKG_VERSION"));
    let curl = Command::new("curl")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match curl {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string();
            check(true, "curl (needed for api update / auth / upload / revert)", &v);
        }
        _ => check(false, "curl (needed for api update / auth / upload / revert)", "not found on PATH"),
    }
    let dir = util::config_dir();
    let writable = fs::create_dir_all(&dir).is_ok() && fs::write(dir.join(".probe"), b"x").is_ok();
    let _ = fs::remove_file(dir.join(".probe"));
    check(writable, "config directory writable", &dir.display().to_string());

    let database = api::Api::load();
    match &database {
        Some(a) => {
            let age = a.age_days().unwrap_or(0);
            check(true, "Roblox API data", &format!("{} classes, {} day(s) old", a.classes.len(), age));
        }
        None => check(false, "Roblox API data", "not installed (stewdio api update)"),
    }
    if let Some(h) = api::stale_hint(database.as_ref()) {
        ui::hint(&h);
    }

    let profiles = auth::Profiles::load();
    let has_key = cred::load(&profiles.default).is_some();
    check(has_key, "Roblox API key", &format!("profile '{}'", profiles.default));

    match Project::current() {
        Ok(p) => {
            check(true, "project", &format!("'{}' ({}) at {}", p.file.project.name, p.file.project.kind, p.root.display()));
            let loaded = crate::fsproj::load(&p.root, p.is_model());
            let rep = crate::validate::validate(&loaded, p.is_model());
            check(
                rep.errors.is_empty(),
                "project validation",
                &format!("{} error(s), {} warning(s)", rep.errors.len(), rep.warnings.len()),
            );
            check(
                p.file.roblox.universe_id.is_some() && p.file.roblox.place_id.is_some(),
                "upload target (roblox.universe_id / place_id)",
                "",
            );
        }
        Err(e) => check(false, "project", &e.message),
    }
    Ok(())
}

// ---------------------------------------------------------------- clean

#[derive(Args)]
pub struct CleanArgs {
    /// Also delete .stewdio/ (editor backups and upload history)
    #[arg(long)]
    pub all: bool,
    /// Do not ask for confirmation
    #[arg(long)]
    pub force: bool,
}

pub fn clean(a: CleanArgs) -> CliResult<()> {
    let project = Project::current()?;
    let mut doomed = Vec::new();
    for xml in [true, false] {
        let p = project.output_path(xml);
        if p.is_file() {
            doomed.push(p);
        }
    }
    let stewdio = project.root.join(".stewdio");
    if a.all && stewdio.exists() {
        doomed.push(stewdio);
    }
    if doomed.is_empty() {
        println!("Nothing to clean");
        return Ok(());
    }
    for d in &doomed {
        println!("  {}", d.display());
    }
    if !a.force && !util::confirm("Delete the paths above?") {
        return Err(CliError::new(GENERAL, "Cancelled"));
    }
    for d in &doomed {
        if d.is_dir() {
            fs::remove_dir_all(d)?;
        } else {
            fs::remove_file(d)?;
        }
    }
    // remove the (now empty) build directory too
    let out_dir = project.root.join(project.settings().output_dir);
    if out_dir != project.root && out_dir.starts_with(&project.root) {
        let _ = fs::remove_dir(&out_dir);
    }
    ui::success("Cleaned");
    Ok(())
}

// ---------------------------------------------------------------- watch

#[derive(Args)]
pub struct WatchArgs {
    #[command(flatten)]
    pub compile: CompileArgs,
}

fn stamp(root: &Path, ignore: &IgnoreSet, out_dir: &str) -> (u128, usize) {
    fn go(dir: &Path, rel: &str, ignore: &IgnoreSet, out_dir: &str, acc: &mut (u128, usize)) {
        let rd = match fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return,
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            let is_dir = p.is_dir();
            if name.starts_with('.') && name != ".stewdioignore" {
                continue;
            }
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if rel.is_empty() && is_dir && name == out_dir {
                continue;
            }
            if ignore.is_ignored(&child, is_dir) {
                continue;
            }
            if is_dir {
                go(&p, &child, ignore, out_dir, acc);
            } else if let Ok(md) = e.metadata() {
                acc.1 += 1;
                if let Ok(t) = md.modified() {
                    let n = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
                    acc.0 = acc.0.max(n);
                }
            }
        }
    }
    let mut acc = (0u128, 0usize);
    go(root, "", ignore, out_dir, &mut acc);
    acc
}

pub fn watch(a: WatchArgs) -> CliResult<()> {
    let mut project = Project::current()?;
    let mut args = a.compile;
    ui::step(&format!("Watching '{}' for changes (Ctrl+C to stop)", project.file.project.name));
    if let Err(e) = commands::build(&project, &args) {
        ui::error(&e.message);
    }
    let mut last = {
        let ig = IgnoreSet::load(&project.root);
        stamp(&project.root, &ig, &project.settings().output_dir)
    };
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let ig = IgnoreSet::load(&project.root);
        let now = stamp(&project.root, &ig, &project.settings().output_dir);
        if now == last {
            continue;
        }
        std::thread::sleep(Duration::from_millis(300)); // let editors finish writing
        last = stamp(&project.root, &ig, &project.settings().output_dir);
        println!();
        ui::step("Change detected, rebuilding...");
        match Project::load(&project.root) {
            Ok(p) => project = p,
            Err(e) => {
                ui::error(&e.message);
                continue;
            }
        }
        args.quiet = false;
        if let Err(e) = commands::build(&project, &args) {
            ui::error(&e.message);
        }
    }
}
