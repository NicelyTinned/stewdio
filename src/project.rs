use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::*;
use crate::util;

pub const FORMAT_VERSION: &str = "1";

pub const DEFAULT_IGNORE: &str = "# Build output
build/

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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CompilerSection {
    pub format: String,
    pub output: Option<String>,
    pub preserve_unknown_properties: bool,
}

impl Default for CompilerSection {
    fn default() -> Self {
        CompilerSection { format: "binary".to_string(), output: None, preserve_unknown_properties: true }
    }
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

pub struct Project {
    pub root: PathBuf,
    pub file: ProjectFile,
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
        if file.project.kind != "place" && file.project.kind != "model" {
            return Err(CliError::new(
                PROJECT,
                format!("Invalid Project.toml: project.type must be \"place\" or \"model\" (got \"{}\")", file.project.kind),
            ));
        }
        if file.compiler.format != "xml" && file.compiler.format != "binary" {
            return Err(CliError::new(
                PROJECT,
                format!("Invalid Project.toml: compiler.format must be \"xml\" or \"binary\" (got \"{}\")", file.compiler.format),
            ));
        }
        Ok(Project { root: root.to_path_buf(), file })
    }

    pub fn output_path(&self, xml_override: Option<bool>) -> PathBuf {
        let use_xml = xml_override.unwrap_or(self.file.compiler.format == "xml");
        let ext = if use_xml { "rbxlx" } else { "rbxl" };
        let mut p = match &self.file.compiler.output {
            Some(o) => self.root.join(o),
            None => self
                .root
                .join("build")
                .join(format!("{}.{}", util::sanitize_name(&self.file.project.name), ext)),
        };
        if xml_override.is_some() && p.extension().and_then(|e| e.to_str()) != Some(ext) {
            p.set_extension(ext);
        }
        p
    }
}

pub fn render_project_toml(name: &str, kind: &str, template: &str) -> String {
    let n = util::toml_escape(name);
    let file = util::toml_escape(&util::sanitize_name(name));
    format!(
        "[project]
name = \"{n}\"
version = \"0.1.0\"
type = \"{kind}\"

[roblox]
# universe_id = 123456789
# place_id = 987654321

[compiler]
format = \"binary\"
output = \"build/{file}.rbxl\"
preserve_unknown_properties = true

[decompiler]
extract_scripts = true
extract_assets = true

[templates]
name = \"{template}\"
"
    )
}
