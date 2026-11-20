use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::api::Mode;
use crate::config::Global;
use crate::error::*;
use crate::rbxl::Compression;
use crate::util;

pub const FORMAT_VERSION: &str = "1";

pub const DEFAULT_IGNORE: &str = "# Build output
build/
.stewdio/

# Temporary files
*.tmp
*.bak

# Editor files
.vscode/
.idea/

# Custom exclusions
PrivateAssets/
UnusedScripts/
";

#[derive(Debug, Deserialize)]
pub struct ProjectFile {
    pub project: ProjectSection,
    #[serde(default)]
    pub roblox: RobloxSection,
    #[serde(default)]
    pub compiler: CompilerSection,
    #[serde(default)]
    pub decompiler: DecompilerSection,
    #[serde(default)]
    pub templates: TemplatesSection,
    #[serde(default)]
    pub assets: AssetsSection,
}

/// Who owns assets uploaded by Stewdio (defaults to the owner of the API key).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AssetsSection {
    pub creator_user_id: Option<u64>,
    pub creator_group_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_kind", rename = "type")]
    pub kind: String,
}

fn default_version() -> String {
    "0.1.0".to_string()
}
fn default_kind() -> String {
    "place".to_string()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RobloxSection {
    pub universe_id: Option<u64>,
    pub place_id: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CompilerSection {
    pub format: Option<String>,
    pub output: Option<String>,
    pub preserve_unknown_properties: Option<bool>,
    pub autofill: Option<String>,
    pub compression: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DecompilerSection {
    pub extract_scripts: bool,
    pub extract_assets: bool,
}

impl Default for DecompilerSection {
    fn default() -> Self {
        DecompilerSection { extract_scripts: true, extract_assets: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TemplatesSection {
    pub name: String,
}

impl Default for TemplatesSection {
    fn default() -> Self {
        TemplatesSection { name: "baseplate".to_string() }
    }
}

/// Effective settings: CLI > Project.toml > global config > built-in defaults.
/// (CLI overrides are applied by the commands themselves.)
pub struct Settings {
    pub format: String,
    pub autofill: Mode,
    pub compression: Compression,
    pub output_dir: String,
}

pub struct Project {
    pub root: PathBuf,
    pub file: ProjectFile,
}

fn one_of(what: &str, v: &str, allowed: &[&str]) -> CliResult<()> {
    if allowed.contains(&v) {
        Ok(())
    } else {
        Err(CliError::new(
            PROJECT,
            format!("Invalid Project.toml: {what} must be one of {} (got \"{v}\")", allowed.join(", ")),
        ))
    }
}

impl Project {
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut cur = Some(start);
        while let Some(d) = cur {
            if d.join("Project.toml").is_file() {
                return Some(d.to_path_buf());
            }
            cur = d.parent();
        }
        None
    }

    pub fn current() -> CliResult<Project> {
        let cwd = std::env::current_dir()?;
        match Self::find_root(&cwd) {
            Some(root) => Self::load(&root),
            None => Err(CliError::new(
                PROJECT,
                "Project.toml not found in this directory or any parent (run `stewdio new` or `stewdio init`)",
            )),
        }
    }

    pub fn load(root: &Path) -> CliResult<Project> {
        let path = root.join("Project.toml");
        let text = fs::read_to_string(&path)
            .map_err(|e| CliError::new(PROJECT, format!("Invalid Project.toml: cannot read ({e})")))?;
        let file: ProjectFile = toml::from_str(&text)
            .map_err(|e| CliError::new(PROJECT, format!("Invalid Project.toml: {e}")))?;
        if file.project.name.trim().is_empty() {
            return Err(CliError::new(PROJECT, "Invalid Project.toml: project.name is empty"));
        }
        one_of("project.type", &file.project.kind, &["place", "model"])?;
        if let Some(v) = &file.compiler.format {
            one_of("compiler.format", v, &["xml", "binary"])?;
        }
        if let Some(v) = &file.compiler.autofill {
            one_of("compiler.autofill", v, &["off", "normalize", "defaults"])?;
        }
        if let Some(v) = &file.compiler.compression {
            one_of("compiler.compression", v, &["automatic", "enabled", "disabled"])?;
        }
        Ok(Project { root: root.to_path_buf(), file })
    }

    pub fn is_model(&self) -> bool {
        self.file.project.kind == "model"
    }

    pub fn settings(&self) -> Settings {
        let g = Global::load();
        let pick = |proj: Option<&String>, key: &str, def: &str| -> String {
            proj.cloned().or_else(|| g.get(key)).unwrap_or_else(|| def.to_string())
        };
        let c = &self.file.compiler;
        Settings {
            format: pick(c.format.as_ref(), "compiler.format", "binary"),
            autofill: Mode::parse(&pick(c.autofill.as_ref(), "compiler.autofill", "normalize")),
            compression: Compression::parse(&pick(c.compression.as_ref(), "compiler.compression", "automatic")),
            output_dir: g.get("compiler.output_dir").unwrap_or_else(|| "build".to_string()),
        }
    }

    pub fn extension(&self, use_xml: bool) -> &'static str {
        match (self.is_model(), use_xml) {
            (false, true) => "rbxlx",
            (false, false) => "rbxl",
            (true, true) => "rbxmx",
            (true, false) => "rbxm",
        }
    }

    /// Output path from Project.toml (or the default), with the extension matching `use_xml`.
    pub fn output_path(&self, use_xml: bool) -> PathBuf {
        let ext = self.extension(use_xml);
        let mut p = match &self.file.compiler.output {
            Some(o) => self.root.join(o),
            None => self
                .root
                .join(self.settings().output_dir)
                .join(format!("{}.{}", util::sanitize_name(&self.file.project.name), ext)),
        };
        if p.extension().and_then(|e| e.to_str()) != Some(ext) {
            p.set_extension(ext);
        }
        p
    }
}

/// Which format does a file extension imply? Some(true) = XML, Some(false) = binary.
pub fn xml_from_ext(p: &Path) -> Option<bool> {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("rbxlx") | Some("rbxmx") => Some(true),
        Some("rbxl") | Some("rbxm") => Some(false),
        _ => None,
    }
}

pub fn render_project_toml(name: &str, kind: &str, template: &str) -> String {
    render_project_toml_with_ids(name, kind, template, None, None)
}

pub fn render_project_toml_with_ids(name: &str, kind: &str, template: &str, universe: Option<u64>, place: Option<u64>) -> String {
    let n = util::toml_escape(name);
    let file = util::toml_escape(&util::sanitize_name(name));
    let ext = if kind == "model" { "rbxm" } else { "rbxl" };
    let uni = match universe {
        Some(u) => format!("universe_id = {u}"),
        None => "# universe_id = 123456789".to_string(),
    };
    let pl = match place {
        Some(p) => format!("place_id = {p}"),
        None => "# place_id = 987654321".to_string(),
    };
    format!(
        "[project]
name = \"{n}\"
version = \"0.1.0\"
type = \"{kind}\"

[roblox]
{uni}
{pl}

[compiler]
format = \"binary\"
output = \"build/{file}.{ext}\"
preserve_unknown_properties = true
# autofill = \"normalize\"   # off | normalize | defaults (needs `stewdio api update`)
# compression = \"automatic\"  # automatic | enabled | disabled

[decompiler]
extract_scripts = true
extract_assets = true

[templates]
name = \"{template}\"

[assets]
# Files in assets/ are uploaded automatically when referenced as file://name.png
# creator_user_id = 123456789    # or creator_group_id; defaults to the API key's owner
"
    )
}
