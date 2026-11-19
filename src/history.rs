//! Local upload history and `stewdio revert`.
//!
//! Open Cloud cannot list or roll back the server-side versions of a place, so Stewdio keeps a
//! snapshot of every file it uploads (`.stewdio/history/`). Reverting re-uploads an old snapshot,
//! which Roblox records as a new version (place version history is append-only).
use clap::Args;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use crate::error::*;
use crate::project::Project;
use crate::{auth, ui, upload, util};

const KEEP: usize = 20;

pub struct Entry {
    pub time: u64,
    pub universe: u64,
    pub place: u64,
    pub version: String,
    pub kind: String,
    pub file: String,
    pub bytes: u64,
}

fn dir(root: &Path) -> PathBuf {
    root.join(".stewdio").join("history")
}

pub fn load(root: &Path) -> Vec<Entry> {
    let text = fs::read_to_string(dir(root).join("index.tsv")).unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 {
            continue;
        }
        if let (Ok(time), Ok(universe), Ok(place), Ok(bytes)) =
            (f[0].parse::<u64>(), f[1].parse::<u64>(), f[2].parse::<u64>(), f[6].parse::<u64>())
        {
            out.push(Entry { time, universe, place, version: f[3].to_string(), kind: f[4].to_string(), file: f[5].to_string(), bytes });
        }
    }
    out
}

fn save_index(root: &Path, entries: &[Entry]) -> io::Result<()> {
    let mut s = String::new();
    for e in entries {
        s.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", e.time, e.universe, e.place, e.version, e.kind, e.file, e.bytes));
    }
    fs::write(dir(root).join("index.tsv"), s)
}

pub fn record(root: &Path, file: &Path, universe: u64, place: u64, version: &str, kind: &str) -> io::Result<()> {
    let d = dir(root);
    fs::create_dir_all(&d)?;
    let time = util::now_secs();
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("rbxl");
    let safe: String = version.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let name = format!("{time}-v{safe}.{ext}");
    fs::copy(file, d.join(&name))?;
    let bytes = fs::metadata(d.join(&name))?.len();
    let mut entries = load(root);
    entries.push(Entry { time, universe, place, version: version.to_string(), kind: kind.to_string(), file: name, bytes });
    while entries.len() > KEEP {
        let old = entries.remove(0);
        let _ = fs::remove_file(d.join(old.file));
    }
    save_index(root, &entries)
}

#[derive(Args)]
pub struct RevertArgs {
    /// Roblox version number to go back to (as printed by `upload` / `--list`)
    pub version: Option<String>,
    /// List the local upload history and exit
    #[arg(long)]
    pub list: bool,
    /// Go back N uploads from the latest (1 = the upload before the latest)
    #[arg(long)]
    pub steps: Option<usize>,
    /// Publish the reverted version (default: save it without publishing)
    #[arg(long)]
    pub publish: bool,
    /// Skip the confirmation prompt
    #[arg(long)]
    pub force: bool,
    /// Authentication profile to use
    #[arg(long)]
    pub profile: Option<String>,
}

fn print_table(entries: &[&Entry]) {
    println!("{:>3}  {:<19}  {:>8}  {:<9}  {:>9}  {}", "#", "uploaded (UTC)", "version", "type", "size", "place");
    for (i, e) in entries.iter().enumerate() {
        println!(
            "{:>3}  {:<19}  {:>8}  {:<9}  {:>9}  {}",
            i + 1,
            util::fmt_time(e.time),
            e.version,
            e.kind,
            upload::human(e.bytes as usize),
            e.place
        );
    }
}

pub fn run(a: RevertArgs) -> CliResult<()> {
    let project = Project::current()?;
    let all = load(&project.root);
    let rb = &project.file.roblox;
    // only entries for this project's place when it is configured
    let entries: Vec<&Entry> = all
        .iter()
        .filter(|e| rb.place_id.map(|p| p == e.place).unwrap_or(true) && rb.universe_id.map(|u| u == e.universe).unwrap_or(true))
        .collect();
    if entries.is_empty() {
        println!("No local upload history yet.");
        ui::hint("every `stewdio upload` keeps a snapshot here, so `stewdio revert` can go back to it later");
        return Ok(());
    }
    // newest first for display
    let newest_first: Vec<&Entry> = entries.iter().rev().copied().collect();
    if a.list {
        print_table(&newest_first);
        return Ok(());
    }

    let target: &Entry = if let Some(v) = &a.version {
        *newest_first
            .iter()
            .find(|e| &e.version == v)
            .ok_or_else(|| CliError::new(ARGS, format!("No local snapshot for version {v} (see `stewdio revert --list`)")))?
    } else if let Some(n) = a.steps {
        *newest_first
            .get(n)
            .ok_or_else(|| CliError::new(ARGS, format!("Only {} snapshot(s) available", newest_first.len())))?
    } else {
        print_table(&newest_first);
        print!("\nRevert to which upload? (number, blank to cancel): ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        let t = line.trim();
        if t.is_empty() {
            return Err(CliError::new(GENERAL, "Cancelled"));
        }
        let n: usize = t.parse().map_err(|_| CliError::new(ARGS, format!("'{t}' is not a number")))?;
        *newest_first
            .get(n.wrapping_sub(1))
            .ok_or_else(|| CliError::new(ARGS, "Number out of range"))?
    };

    let file = dir(&project.root).join(&target.file);
    if !file.is_file() {
        return Err(CliError::new(GENERAL, format!("Snapshot file {} is missing", file.display())));
    }
    let (is_binary, size) = upload::sniff(&file)?;
    let key = auth::current_key(a.profile.as_deref()).ok_or_else(|| {
        CliError::new(AUTH, "Roblox authentication failed: no API key configured (run `stewdio auth login`)")
    })?;
    let kind = if a.publish { "Published" } else { "Saved" };
    println!("Revert to: version {} uploaded {}", target.version, util::fmt_time(target.time));
    println!("Size:      {}", upload::human(size));
    println!("Place:     {} (universe {})", target.place, target.universe);
    println!("Result:    a NEW version containing the old content ({kind})");
    if newest_first.first().map(|e| std::ptr::eq(*e, target)).unwrap_or(false) {
        ui::warning("this is already the latest upload");
    }
    if !a.force && !util::confirm("Upload this snapshot as a new version?") {
        return Err(CliError::new(GENERAL, "Cancelled"));
    }
    ui::step("Uploading...");
    let version = upload::send(&file, is_binary, target.universe, target.place, a.publish, key)?;
    ui::success("Reverted!");
    println!("New version number: {version} ({kind})");
    let (u, p) = (target.universe, target.place);
    if let Err(e) = record(&project.root, &file, u, p, &version, kind) {
        ui::warning(&format!("could not record the new snapshot: {e}"));
    }
    Ok(())
}
