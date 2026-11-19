//! `stewdio diff`: compare two place/model files or project directories.
use clap::Args;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands;
use crate::error::*;
use crate::fsproj;
use crate::model::*;
use crate::project::Project;
use crate::ui;

#[derive(Args)]
pub struct DiffArgs {
    /// First file or project directory
    pub a: PathBuf,
    /// Second file or project directory
    pub b: PathBuf,
    /// Only compare instances, not properties or scripts
    #[arg(long)]
    pub no_props: bool,
    /// Exit with code 1 when differences exist
    #[arg(long)]
    pub exit_code: bool,
}

fn load_any(p: &Path) -> CliResult<Vec<Instance>> {
    if p.is_dir() {
        let model = Project::load(p).map(|pr| pr.is_model()).unwrap_or(false);
        let l = fsproj::load(p, model);
        if let Some(e) = l.diags.iter().find(|d| d.is_error) {
            return Err(CliError::new(PROJECT, format!("{}: {}", p.display(), e.msg)));
        }
        Ok(l.roots)
    } else if p.is_file() {
        let data = fs::read(p)?;
        Ok(commands::decode_bytes(&data)?.roots)
    } else {
        Err(CliError::new(ARGS, format!("'{}' does not exist", p.display())))
    }
}

struct Side<'a> {
    items: BTreeMap<String, &'a Instance>,
    ids: HashMap<String, String>,
}

fn flatten<'a>(list: &'a [Instance], prefix: &str, side: &mut Side<'a>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for i in list {
        let n = seen.entry(i.name.as_str()).or_insert(0);
        *n += 1;
        let key = if *n == 1 { i.name.clone() } else { format!("{}#{}", i.name, *n) };
        let path = if prefix.is_empty() { key } else { format!("{prefix}/{key}") };
        side.ids.insert(i.id.clone(), path.clone());
        side.items.insert(path.clone(), i);
        flatten(&i.children, &path, side);
    }
}

fn sum(v: &PropValue) -> String {
    match v {
        PropValue::Text(t) => t.clone(),
        PropValue::Fields(f) => f
            .iter()
            .map(|(k, x)| format!("{k}={}", sum(x)))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn clip(s: &str) -> String {
    let s = s.replace('\n', "⏎");
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s
    }
}

fn prop_map(inst: &Instance, ids: &HashMap<String, String>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for p in &inst.properties {
        let v = if p.ty == "Ref" {
            match &p.value {
                PropValue::Text(t) if t == "null" => "null".to_string(),
                PropValue::Text(t) => format!("-> {}", ids.get(t).cloned().unwrap_or_else(|| "?".to_string())),
                other => sum(other),
            }
        } else {
            sum(&p.value)
        };
        m.insert(p.name.clone(), format!("{v} <{}>", p.ty));
    }
    m
}

fn line_delta(a: &str, b: &str) -> (usize, usize) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in a.lines() {
        *counts.entry(l).or_insert(0) -= 1;
    }
    for l in b.lines() {
        *counts.entry(l).or_insert(0) += 1;
    }
    let added = counts.values().filter(|v| **v > 0).map(|v| *v as usize).sum();
    let removed = counts.values().filter(|v| **v < 0).map(|v| (-*v) as usize).sum();
    (added, removed)
}

pub fn run(a: DiffArgs) -> CliResult<()> {
    let ra = load_any(&a.a)?;
    let rb = load_any(&a.b)?;
    let mut sa = Side { items: BTreeMap::new(), ids: HashMap::new() };
    let mut sb = Side { items: BTreeMap::new(), ids: HashMap::new() };
    flatten(&ra, "", &mut sa);
    flatten(&rb, "", &mut sb);

    let mut added = 0;
    let mut removed = 0;
    let mut changed = 0;
    let keys: BTreeSet<&String> = sa.items.keys().chain(sb.items.keys()).collect();
    for k in keys {
        match (sa.items.get(k), sb.items.get(k)) {
            (Some(x), None) => {
                removed += 1;
                println!("{} {k} ({})", ui::paint("31", "-", false), x.class);
            }
            (None, Some(y)) => {
                added += 1;
                println!("{} {k} ({})", ui::paint("32", "+", false), y.class);
            }
            (Some(x), Some(y)) => {
                let mut lines: Vec<String> = Vec::new();
                if x.class != y.class {
                    lines.push(format!("class: {} -> {}", x.class, y.class));
                }
                if !a.no_props {
                    let pa = prop_map(x, &sa.ids);
                    let pb = prop_map(y, &sb.ids);
                    let names: BTreeSet<&String> = pa.keys().chain(pb.keys()).collect();
                    for n in names {
                        match (pa.get(n), pb.get(n)) {
                            (Some(v), None) => lines.push(format!("-{n} = {}", clip(v))),
                            (None, Some(v)) => lines.push(format!("+{n} = {}", clip(v))),
                            (Some(v1), Some(v2)) if v1 != v2 => lines.push(format!("~{n}: {} -> {}", clip(v1), clip(v2))),
                            _ => {}
                        }
                    }
                    if x.tags != y.tags {
                        lines.push(format!("Tags: [{}] -> [{}]", x.tags.join(", "), y.tags.join(", ")));
                    }
                    if x.source != y.source {
                        let (ad, rm) = line_delta(x.source.as_deref().unwrap_or(""), y.source.as_deref().unwrap_or(""));
                        lines.push(format!("Source: +{ad} -{rm} line(s)"));
                    }
                }
                if !lines.is_empty() {
                    changed += 1;
                    println!("{} {k} ({})", ui::paint("33", "~", false), y.class);
                    for l in lines {
                        println!("    {l}");
                    }
                }
            }
            (None, None) => {}
        }
    }
    println!("\n{added} added, {removed} removed, {changed} changed");
    if a.exit_code && (added + removed + changed) > 0 {
        return Err(CliError::new(GENERAL, "Differences found"));
    }
    Ok(())
}
