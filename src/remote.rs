//! `stewdio from` and `stewdio pull`: fetch a place from Roblox with Open Cloud's asset delivery API.
//!
//! The API key needs the "legacy-asset:manage" permission (Creator Hub > Open Cloud > API Keys) and
//! the place must belong to the key's owner.
use clap::Args;
use std::fs;
use std::path::PathBuf;

use crate::commands;
use crate::error::*;
use crate::project::Project;
use crate::{auth, fsproj, history, hjson, http, ui, upload, util};

fn require_key(profile: Option<&str>) -> CliResult<String> {
    auth::current_key(profile).ok_or_else(|| {
        CliError::new(AUTH, "Roblox authentication failed: no API key configured (run `stewdio auth login`)")
    })
}

/// Download the raw place file (binary .rbxl or XML) of `place`.
pub fn download_place(place: u64, version: Option<u64>, key: &str) -> CliResult<Vec<u8>> {
    let url = match version {
        Some(v) => format!("https://apis.roblox.com/asset-delivery-api/v1/assetId/{place}/version/{v}"),
        None => format!("https://apis.roblox.com/asset-delivery-api/v1/assetId/{place}"),
    };
    let resp = http::request(
        "GET",
        &url,
        &[("x-api-key", key.to_string()), ("Accept", "application/json".to_string())],
        None,
        None,
    )
    .map_err(|e| CliError::new(GENERAL, e))?;
    match resp.status {
        200 => {}
        401 => return Err(CliError::new(AUTH, "Roblox authentication failed (HTTP 401): the API key is invalid, disabled or expired")),
        403 => {
            return Err(CliError::new(
                AUTH,
                "Permission denied (HTTP 403): the API key needs the \"legacy-asset:manage\" permission, the place must belong to the key's owner, and your IP must be allowed",
            ))
        }
        404 => return Err(CliError::new(GENERAL, format!("Place {place} was not found (or is not accessible with this key)"))),
        429 => return Err(CliError::new(GENERAL, "Rate limited by Roblox (HTTP 429): try again in a minute")),
        s => return Err(CliError::new(GENERAL, format!("Roblox returned HTTP {s} while requesting the place"))),
    }
    let body = String::from_utf8_lossy(&resp.body).to_string();
    let v = hjson::parse(&body).map_err(|_| CliError::new(PARSE, "Unexpected response from Roblox"))?;
    let location = v
        .get("location")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("locations")
                .and_then(|l| l.as_array())
                .and_then(|a| a.first())
                .and_then(|f| f.get("location"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| CliError::new(GENERAL, "Roblox did not return a download location for this place"))?;
    ui::step("Downloading place file...");
    let file = http::request("GET", &location, &[], None, None).map_err(|e| CliError::new(GENERAL, e))?;
    if file.status != 200 {
        return Err(CliError::new(GENERAL, format!("Download failed (HTTP {})", file.status)));
    }
    Ok(file.body)
}

fn place_name(universe: u64, place: u64, key: &str) -> Option<String> {
    let url = format!("https://apis.roblox.com/cloud/v2/universes/{universe}/places/{place}");
    let r = http::request("GET", &url, &[("x-api-key", key.to_string())], None, None).ok()?;
    if r.status != 200 {
        return None;
    }
    let v = hjson::parse(&String::from_utf8_lossy(&r.body)).ok()?;
    v.get("displayName").and_then(|x| x.as_str()).map(|s| s.to_string())
}

#[derive(Args)]
pub struct FromArgs {
    /// Universe ID of the experience
    pub universe: u64,
    /// Place ID to download
    pub place: u64,
    /// Project name (default: the place's display name)
    #[arg(long)]
    pub name: Option<String>,
    /// Directory to create the project in (default: current directory)
    #[arg(long)]
    pub path: Option<PathBuf>,
    /// Download a specific place version instead of the latest
    #[arg(long)]
    pub version: Option<u64>,
    /// Do not export script source
    #[arg(long)]
    pub no_scripts: bool,
    /// Authentication profile to use
    #[arg(long)]
    pub profile: Option<String>,
}

pub fn from(a: FromArgs) -> CliResult<()> {
    let key = require_key(a.profile.as_deref())?;
    let name = match &a.name {
        Some(n) => n.clone(),
        None => {
            ui::step("Looking up the place name...");
            place_name(a.universe, a.place, &key).unwrap_or_else(|| format!("place-{}", a.place))
        }
    };
    let out = a.path.clone().unwrap_or_else(|| PathBuf::from(".")).join(util::sanitize_name(&name));
    commands::prepare_output_dir(&out, false)?;
    ui::step(&format!("Requesting place {} ...", a.place));
    let data = download_place(a.place, a.version, &key)?;
    ui::step(&format!("Downloaded {}", upload::human(data.len())));
    let stats = commands::import_bytes(&data, &name, &out, false, false, a.no_scripts, Some(a.universe), Some(a.place))?;

    // keep the download as the baseline snapshot so `stewdio revert` can return to it
    let tmp_dir = out.join(".stewdio");
    fs::create_dir_all(&tmp_dir)?;
    let tmp = tmp_dir.join(if data.starts_with(b"<roblox!") { "download.rbxl" } else { "download.rbxlx" });
    fs::write(&tmp, &data)?;
    let _ = history::record(&out, &tmp, a.universe, a.place, "base", "Downloaded");
    let _ = fs::remove_file(&tmp);

    ui::success("\nProject created!\n");
    println!("Name: {name}");
    println!("Instances: {}", stats.instances);
    println!("Scripts: {}", stats.scripts);
    println!("Warnings: {}", stats.warnings.len());
    println!("Output: {}", out.display());
    println!("\nNext steps:\n  cd {}\n  stewdio explorer", out.display());
    Ok(())
}

#[derive(Args)]
pub struct PullArgs {
    /// Download a specific place version instead of the latest
    #[arg(long)]
    pub version: Option<u64>,
    /// Skip the confirmation prompt
    #[arg(long)]
    pub force: bool,
    /// Authentication profile to use
    #[arg(long)]
    pub profile: Option<String>,
}

/// Replace the current project's instances with the latest copy of its place on Roblox.
pub fn pull(a: PullArgs) -> CliResult<()> {
    let project = Project::current()?;
    if project.is_model() {
        return Err(CliError::new(UNSUPPORTED, "`pull` only supports place projects"));
    }
    let place = match project.file.roblox.place_id {
        Some(p) => p,
        None => upload::ask_id("Place ID")?,
    };
    let key = require_key(a.profile.as_deref())?;
    ui::step(&format!("Requesting place {place} ..."));
    let data = download_place(place, a.version, &key)?;
    let decoded = commands::decode_bytes(&data)?;
    let incoming: usize = decoded.roots.iter().map(|r| r.count()).sum();
    let current = fsproj::load(&project.root, false);
    let existing: usize = current.roots.iter().map(|r| r.count()).sum();
    println!("Downloaded {} ({} instances; the project has {})", upload::human(data.len()), incoming, existing);
    if !a.force
        && !util::confirm("Replace the project's instances with the downloaded place? (the old ones are backed up in .stewdio/backup)")
    {
        return Err(CliError::new(GENERAL, "Cancelled"));
    }
    let backup = fsproj::save_project(&project.root, false, &decoded.roots)?;
    ui::success("Pulled the latest place");
    if let Some(b) = backup {
        println!("Previous instances backed up in {}", b.display());
    }
    for w in decoded.warnings.iter().take(5) {
        ui::warning(w);
    }
    Ok(())
}
