//! `stewdio upload`: publish a compiled place with Roblox Open Cloud.
use clap::Args;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::commands::{self, CompileArgs};
use crate::error::*;
use crate::project::{xml_from_ext, Project};
use crate::{auth, hjson, http, ui, util};

#[derive(Args)]
pub struct UploadArgs {
    /// Upload an existing compiled place file
    #[arg(long)]
    pub file: Option<PathBuf>,
    /// Specify the universe ID
    #[arg(long)]
    pub universe: Option<u64>,
    /// Specify the place ID
    #[arg(long)]
    pub place: Option<u64>,
    /// Skip the upload confirmation prompt
    #[arg(long)]
    pub force: bool,
    /// Publish the new version (default: save it without publishing)
    #[arg(long)]
    pub publish: bool,
    /// Authentication profile to use
    #[arg(long)]
    pub profile: Option<String>,
}

fn ask_id(what: &str) -> CliResult<u64> {
    print!("{what}: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        return Err(CliError::new(ARGS, format!("{what} is required (use the flag or set it in Project.toml)")));
    }
    line.trim()
        .parse::<u64>()
        .map_err(|_| CliError::new(ARGS, format!("'{}' is not a valid {what}", line.trim())))
}

pub fn run(a: UploadArgs) -> CliResult<()> {
    let project = Project::current().ok();
    if let Some(p) = &project {
        if p.is_model() && a.file.is_none() {
            return Err(CliError::new(UNSUPPORTED, "Uploading model projects is not supported; Open Cloud place publishing only accepts places"));
        }
    }

    // --- locate or build the file
    let file = match &a.file {
        Some(f) => f.clone(),
        None => {
            let p = project.as_ref().ok_or_else(|| {
                CliError::new(PROJECT, "No Project.toml found; run inside a project or pass --file")
            })?;
            ui::step("Compiling project...");
            let built = commands::build(
                p,
                &CompileArgs { xml: false, binary: false, output: None, release: false, verbose: false, quiet: true, no_cache: false, autofill: None },
            )?;
            built.out
        }
    };
    if !file.is_file() {
        return Err(CliError::new(ARGS, format!("File '{}' not found", file.display())));
    }

    // --- make sure the content matches what the extension claims (and what we tell Roblox)
    let data = fs::read(&file)?;
    let is_binary = data.starts_with(b"<roblox!");
    let head = String::from_utf8_lossy(&data[..data.len().min(64)]).to_string();
    let is_xml = head.trim_start_matches('\u{feff}').trim_start().starts_with("<roblox");
    if !is_binary && !is_xml {
        return Err(CliError::new(UNSUPPORTED, "The file is not a Roblox place file"));
    }
    match file.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("rbxm") | Some("rbxmx") => {
            return Err(CliError::new(UNSUPPORTED, "Model files cannot be uploaded as places"));
        }
        _ => {}
    }
    if let Some(ext_xml) = xml_from_ext(&file) {
        if ext_xml && is_binary {
            return Err(CliError::new(UPLOAD, "The file has an .rbxlx extension but contains binary data; recompile with --xml or rename it to .rbxl"));
        }
        if !ext_xml && !is_binary {
            return Err(CliError::new(UPLOAD, "The file has an .rbxl extension but contains XML; recompile with --binary or rename it to .rbxlx"));
        }
    }
    let content_type = if is_binary { "application/octet-stream" } else { "application/xml" };

    // --- ids
    let rb = project.as_ref().map(|p| &p.file.roblox);
    let universe = match a.universe.or(rb.and_then(|r| r.universe_id)) {
        Some(u) => u,
        None => ask_id("Universe ID")?,
    };
    let place = match a.place.or(rb.and_then(|r| r.place_id)) {
        Some(p) => p,
        None => ask_id("Place ID")?,
    };

    // --- credentials
    let key = auth::current_key(a.profile.as_deref())
        .ok_or_else(|| CliError::new(AUTH, "Roblox authentication failed: no API key configured (run `stewdio auth login`)"))?;

    let version_type = if a.publish { "Published" } else { "Saved" };
    println!("File:      {} ({})", file.display(), if is_binary { "binary" } else { "XML" });
    println!("Size:      {}", human(data.len()));
    println!("Universe:  {universe}");
    println!("Place:     {place}");
    println!("Version:   {version_type}{}", if a.publish { " (goes live)" } else { " (not published)" });
    if !a.force && !util::confirm("Upload this place to Roblox?") {
        return Err(CliError::new(GENERAL, "Upload cancelled"));
    }

    ui::step("Uploading...");
    let url = format!("https://apis.roblox.com/universes/v1/{universe}/places/{place}/versions?versionType={version_type}");
    let resp = http::request(
        "POST",
        &url,
        &[("x-api-key", key), ("Content-Type", content_type.to_string())],
        Some(file.as_path()),
        None,
    )
    .map_err(|e| CliError::new(UPLOAD, e))?;

    let body = String::from_utf8_lossy(&resp.body).to_string();
    match resp.status {
        200 => {
            let ver = hjson::parse(&body)
                .ok()
                .and_then(|v| v.get("versionNumber").cloned())
                .map(|v| match v {
                    hjson::Value::Number(n) => n,
                    other => format!("{other:?}"),
                })
                .unwrap_or_else(|| "?".to_string());
            ui::success("Upload successful!");
            println!("Version number: {ver} ({version_type})");
            if !a.publish {
                ui::hint("the version was saved but not published; re-run with --publish to make it live");
            }
            Ok(())
        }
        401 => Err(CliError::new(AUTH, "Roblox authentication failed (HTTP 401): the API key is invalid, disabled or expired")),
        403 => Err(CliError::new(UPLOAD, "Upload permission denied (HTTP 403): the key needs the universe-places:write permission for this experience, and your IP must be allowed")),
        404 => Err(CliError::new(UPLOAD, "Universe or place not found (HTTP 404): check the IDs")),
        429 => Err(CliError::new(UPLOAD, "Rate limited by Roblox (HTTP 429): try again in a minute")),
        s => {
            let msg = hjson::parse(&body)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str().map(|x| x.to_string())))
                .unwrap_or_else(|| body.chars().take(300).collect());
            Err(CliError::new(UPLOAD, format!("Roblox rejected the upload (HTTP {s}): {msg}")))
        }
    }
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
