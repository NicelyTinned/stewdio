use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::*;
use crate::fsproj::{self, WriteOptions};
use crate::model::*;
use crate::project::{self, Project};
use crate::{rbxlx, util, validate};

fn say(quiet: bool, msg: &str) {
    if !quiet {
        println!("{msg}");
    }
}

fn dir_nonempty(p: &Path) -> bool {
    p.is_dir() && fs::read_dir(p).map(|mut r| r.next().is_some()).unwrap_or(false)
}

// ------------------------------------------------------------------ new

#[derive(Args)]
pub struct NewArgs {
    /// Project name (also used as the directory name)
    pub name: String,
    /// Create a model project instead of a place project
    #[arg(long)]
    pub model: bool,
    /// Select a project template (baseplate, empty)
    #[arg(long)]
    pub template: Option<String>,
    /// Create an empty project without the Baseplate template
    #[arg(long)]
    pub empty: bool,
    /// Specify the output directory
    #[arg(long)]
    pub path: Option<PathBuf>,
}

fn t(name: &str, ty: &str, v: &str) -> Property {
    Property { name: name.to_string(), ty: ty.to_string(), value: PropValue::Text(v.to_string()) }
}

fn fields(items: &[(&str, &str)]) -> PropValue {
    PropValue::Fields(items.iter().map(|(k, v)| (k.to_string(), PropValue::Text(v.to_string()))).collect())
}

fn baseplate_props() -> Vec<Property> {
    vec![
        t("Anchored", "bool", "true"),
        t("CanCollide", "bool", "true"),
        t("Locked", "bool", "true"),
        t("Color3uint8", "Color3uint8", "4288914085"),
        // Studio serializes Part.Size under its legacy name "size".
        Property {
            name: "size".to_string(),
            ty: "Vector3".to_string(),
            value: fields(&[("X", "512"), ("Y", "1"), ("Z", "512")]),
        },
        Property {
            name: "CFrame".to_string(),
            ty: "CoordinateFrame".to_string(),
            value: fields(&[
                ("X", "0"),
                ("Y", "-0.5"),
                ("Z", "0"),
                ("R00", "1"),
                ("R01", "0"),
                ("R02", "0"),
                ("R10", "0"),
                ("R11", "1"),
                ("R12", "0"),
                ("R20", "0"),
                ("R21", "0"),
                ("R22", "1"),
            ]),
        },
    ]
}

fn default_tree(baseplate: bool) -> Vec<Instance> {
    let mut n = 0usize;
    let mut mk = |class: &str| -> Instance {
        n += 1;
        Instance::new(&format!("instance-{n:03}"), class, class)
    };
    let mut workspace = mk("Workspace");
    if baseplate {
        let mut bp = mk("Part");
        bp.name = "Baseplate".to_string();
        bp.properties = baseplate_props();
        workspace.children.push(bp);
    }
    let mut sp = mk("StarterPlayer");
    sp.children.push(mk("StarterCharacterScripts"));
    sp.children.push(mk("StarterPlayerScripts"));
    let mut v = vec![workspace];
    for c in [
        "Players",
        "Lighting",
        "ReplicatedFirst",
        "ReplicatedStorage",
        "ServerScriptService",
        "ServerStorage",
        "StarterGui",
        "StarterPack",
    ] {
        v.push(mk(c));
    }
    v.push(sp);
    v.push(mk("SoundService"));
    v
}

pub fn new_project(a: NewArgs) -> CliResult<()> {
    if a.model {
        return Err(CliError::new(UNSUPPORTED, "Model projects are not supported until v0.3"));
    }
    let template = if a.empty {
        "empty".to_string()
    } else {
        a.template.clone().unwrap_or_else(|| "baseplate".to_string())
    };
    if template != "baseplate" && template != "empty" {
        return Err(CliError::new(
            ARGS,
            format!("Unknown template '{template}' (available: baseplate, empty)"),
        ));
    }
    let dir = a.path.clone().unwrap_or_else(|| PathBuf::from(".")).join(&a.name);
    if dir_nonempty(&dir) {
        return Err(CliError::new(
            GENERAL,
            format!("Directory '{}' already exists and is not empty", dir.display()),
        ));
    }
    fs::create_dir_all(&dir)?;
    let project_name = Path::new(&a.name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| a.name.clone());

    let roots = default_tree(template == "baseplate");
    let mut warnings = Vec::new();
    fsproj::write_children(&dir, &roots, &WriteOptions { write_scripts: true }, &mut warnings)?;
    fs::write(dir.join("Project.toml"), project::render_project_toml(&project_name, "place", &template))?;
    fs::write(dir.join(".stewdioignore"), project::DEFAULT_IGNORE)?;

    println!("Created project '{}' at {}", project_name, dir.display());
    println!("Template: {template}");
    println!("\nNext steps:\n  cd {}\n  stewdio compile", dir.display());
    Ok(())
}

// ----------------------------------------------------------------- init

#[derive(Args)]
pub struct InitArgs {
    /// Initialize as a model project
    #[arg(long)]
    pub model: bool,
    /// Allow overwriting generated configuration files after confirmation
    #[arg(long)]
    pub force: bool,
}

fn write_guarded(path: &Path, content: &str, force: bool) -> CliResult<&'static str> {
    if path.exists() {
        if force && util::confirm(&format!("Overwrite {}?", path.display())) {
            fs::write(path, content)?;
            return Ok("overwritten");
        }
        return Ok("kept existing file");
    }
    fs::write(path, content)?;
    Ok("created")
}

pub fn init_project(a: InitArgs) -> CliResult<()> {
    if a.model {
        return Err(CliError::new(UNSUPPORTED, "Model projects are not supported until v0.3"));
    }
    let cwd = std::env::current_dir()?;
    let name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "stewdio-project".to_string());

    let r1 = write_guarded(&cwd.join("Project.toml"), &project::render_project_toml(&name, "place", "baseplate"), a.force)?;
    println!("Project.toml: {r1}");
    let r2 = write_guarded(&cwd.join(".stewdioignore"), project::DEFAULT_IGNORE, a.force)?;
    println!(".stewdioignore: {r2}");

    let mut existing = 0usize;
    let mut created = 0usize;
    for e in fs::read_dir(&cwd)?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let dname = e.file_name().to_string_lossy().to_string();
        if p.join("Instance.data.hjson").is_file() {
            existing += 1;
        } else if SERVICES.contains(&dname.as_str()) {
            let inst = Instance::new(&format!("init-{dname}"), &dname, &dname);
            fsproj::write_meta(&p, &inst)?;
            created += 1;
        }
    }
    println!("Detected {existing} existing instance director{}", if existing == 1 { "y" } else { "ies" });
    if created > 0 {
        println!("Generated metadata for {created} service director{}", if created == 1 { "y" } else { "ies" });
    }
    Ok(())
}

// ------------------------------------------------------------- compile

#[derive(Args)]
pub struct CompileArgs {
    /// Compile into .rbxlx format
    #[arg(long, conflicts_with = "binary")]
    pub xml: bool,
    /// Compile into .rbxl format (not available until v0.2)
    #[arg(long)]
    pub binary: bool,
    /// Specify the output file
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Perform release-oriented validation (warnings are errors)
    #[arg(long)]
    pub release: bool,
    /// Display detailed compilation information
    #[arg(long)]
    pub verbose: bool,
    /// Suppress non-error output
    #[arg(long)]
    pub quiet: bool,
    /// Disable compilation caching (v0.1 has no cache; accepted for compatibility)
    #[arg(long)]
    pub no_cache: bool,
}

pub fn compile(a: CompileArgs) -> CliResult<()> {
    let project = Project::current()?;
    let xml_override = if a.binary {
        Some(false)
    } else if a.xml {
        Some(true)
    } else {
        None
    };
    let use_xml = xml_override.unwrap_or(project.file.compiler.format == "xml");
    if !use_xml {
        return Err(CliError::new(
            UNSUPPORTED,
            "Binary (.rbxl) output arrives in v0.2. Use --xml or set compiler.format = \"xml\" in Project.toml",
        ));
    }
    let q = a.quiet;
    say(q, "[Stewdio] Loading project...");
    let loaded = fsproj::load(&project.root);
    say(q, "[Stewdio] Reading instance metadata...");
    say(q, "[Stewdio] Loading Luau scripts...");
    say(q, "[Stewdio] Resolving references...");
    let report = validate::validate(&loaded);
    let release_failures = if a.release { report.warnings.len() } else { 0 };
    if !report.errors.is_empty() || release_failures > 0 {
        for e in &report.errors {
            eprintln!("[Error] {e}");
        }
        if a.release {
            for w in &report.warnings {
                eprintln!("[Error] (release) {w}");
            }
        }
        return Err(CliError::new(
            PROJECT,
            format!("Project validation failed with {} error(s)", report.errors.len() + release_failures),
        ));
    }
    if a.verbose && !q {
        for w in &report.warnings {
            eprintln!("[Warning] {w}");
        }
        println!("[Stewdio] Paths excluded by .stewdioignore: {}", loaded.ignored);
    }
    say(q, "[Stewdio] Building DataModel...");
    say(q, "[Stewdio] Serializing XML output...");
    let xml = rbxlx::encode(&loaded.roots).map_err(|e| CliError::new(COMPILE, e))?;
    let out = a.output.clone().unwrap_or_else(|| project.output_path(xml_override));
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&out, xml)?;
    if !q {
        let instances: usize = loaded.roots.iter().map(|r| r.count()).sum();
        let scripts: usize = loaded.roots.iter().map(|r| r.script_count()).sum();
        println!("\nCompilation successful!\n");
        println!("Output: {}", out.display());
        println!("Instances: {instances}");
        println!("Scripts: {scripts}");
        println!("Warnings: {}", report.warnings.len());
    }
    Ok(())
}

// ----------------------------------------------------------- decompile

#[derive(Args)]
pub struct DecompileArgs {
    /// Roblox place file (.rbxlx; .rbxl arrives in v0.2)
    pub input: PathBuf,
    /// Specify the output directory
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Export as a model project (not available until v0.3)
    #[arg(long)]
    pub model: bool,
    /// Do not export script source
    #[arg(long)]
    pub no_scripts: bool,
    /// Skip external asset extraction (nothing to extract in v0.1)
    #[arg(long)]
    pub no_assets: bool,
    /// Display detailed extraction information
    #[arg(long)]
    pub verbose: bool,
    /// Allow overwriting an existing output directory after confirmation
    #[arg(long)]
    pub overwrite: bool,
}

pub fn decompile(a: DecompileArgs) -> CliResult<()> {
    if a.model {
        return Err(CliError::new(UNSUPPORTED, "Model projects are not supported until v0.3"));
    }
    if !a.input.is_file() {
        return Err(CliError::new(ARGS, format!("Input file '{}' not found", a.input.display())));
    }
    println!("[Stewdio] Reading {}...", a.input.display());
    let data = fs::read(&a.input)?;
    if data.starts_with(b"<roblox!") {
        println!("[Stewdio] Detecting binary format...");
        return Err(CliError::new(
            UNSUPPORTED,
            "Binary .rbxl parsing arrives in v0.2. Re-save the place as .rbxlx for now",
        ));
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(64)]).to_string();
    let head = head.trim_start_matches('\u{feff}').trim_start().to_string();
    if !(head.starts_with("<roblox") || head.starts_with("<?xml")) {
        return Err(CliError::new(UNSUPPORTED, "Unsupported input file format"));
    }
    println!("[Stewdio] Detecting XML format...");
    let text = String::from_utf8(data).map_err(|_| CliError::new(PARSE, "Input file is not valid UTF-8"))?;

    let stem = a
        .input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    let out = a.output.clone().unwrap_or_else(|| PathBuf::from(".").join(util::sanitize_name(&stem)));

    if out.is_file() {
        return Err(CliError::new(GENERAL, format!("Output path '{}' is a file", out.display())));
    }
    if dir_nonempty(&out) {
        if !a.overwrite {
            return Err(CliError::new(
                GENERAL,
                format!("Output directory '{}' already exists and is not empty (use --overwrite)", out.display()),
            ));
        }
        let canon = fs::canonicalize(&out)?;
        let cwd = fs::canonicalize(std::env::current_dir()?)?;
        if cwd.starts_with(&canon) || canon.parent().is_none() {
            return Err(CliError::new(
                GENERAL,
                "Refusing to overwrite the current directory, one of its parents, or the filesystem root",
            ));
        }
        if !util::confirm(&format!("Delete the existing contents of '{}' and continue?", out.display())) {
            return Err(CliError::new(GENERAL, "Aborted"));
        }
        fs::remove_dir_all(&out)?;
    }

    println!("[Stewdio] Parsing XML...");
    let decoded = rbxlx::decode(&text).map_err(|e| CliError::new(PARSE, e))?;
    println!("[Stewdio] Reconstructing instances...");
    println!("[Stewdio] Extracting properties...");
    println!("[Stewdio] Extracting scripts...");
    println!("[Stewdio] Writing project files...");
    fs::create_dir_all(&out)?;
    let mut warnings = decoded.warnings.clone();
    fsproj::write_children(&out, &decoded.roots, &WriteOptions { write_scripts: !a.no_scripts }, &mut warnings)?;
    fs::write(out.join("Project.toml"), project::render_project_toml(&stem, "place", "baseplate"))?;
    fs::write(out.join(".stewdioignore"), project::DEFAULT_IGNORE)?;

    if a.verbose {
        for w in &warnings {
            eprintln!("[Warning] {w}");
        }
    }
    let instances: usize = decoded.roots.iter().map(|r| r.count()).sum();
    let scripts: usize = decoded.roots.iter().map(|r| r.script_count()).sum();
    println!("\nDecompilation successful!\n");
    println!("Instances: {instances}");
    println!("Scripts: {scripts}");
    println!("Warnings: {}", warnings.len());
    println!("Output: {}", out.display());
    Ok(())
}

// ------------------------------------------------------------ validate

#[derive(Args)]
pub struct ValidateArgs {
    /// Treat warnings as errors
    #[arg(long)]
    pub strict: bool,
    /// Display detailed validation output
    #[arg(long)]
    pub verbose: bool,
}

pub fn validate_project(a: ValidateArgs) -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root);
    let report = validate::validate(&loaded);
    if a.verbose {
        println!("[Stewdio] Project.toml: OK");
        println!("[Stewdio] Checked instance metadata, references, property types, class names,");
        println!("[Stewdio] duplicate identifiers, filesystem structure and script files.");
        println!("[Stewdio] Paths excluded by .stewdioignore: {}", loaded.ignored);
    }
    for e in &report.errors {
        eprintln!("[Error] {e}");
    }
    for w in &report.warnings {
        if a.strict {
            eprintln!("[Error] {w}");
        } else {
            eprintln!("[Warning] {w}");
        }
    }
    let errors = report.errors.len() + if a.strict { report.warnings.len() } else { 0 };
    if errors > 0 {
        return Err(CliError::new(PROJECT, format!("Validation failed with {errors} error(s)")));
    }
    println!("Validation passed ({} warning(s))", report.warnings.len());
    Ok(())
}

// ---------------------------------------------------------------- info

pub fn info() -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root);
    let instances: usize = loaded.roots.iter().map(|r| r.count()).sum();
    let scripts: usize = loaded.roots.iter().map(|r| r.script_count()).sum();
    let opt = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "(not set)".to_string());
    println!("Project name:       {}", project.file.project.name);
    println!("Project version:    {}", project.file.project.version);
    println!("Project type:       {}", project.file.project.kind);
    println!("Stewdio format:     {}", project::FORMAT_VERSION);
    println!("Compile format:     {}", project.file.compiler.format);
    println!("Universe ID:        {}", opt(project.file.roblox.universe_id));
    println!("Place ID:           {}", opt(project.file.roblox.place_id));
    println!("Instances:          {instances}");
    println!("Scripts:            {scripts}");
    println!("Project directory:  {}", project.root.display());
    Ok(())
}

// --------------------------------------------------------------- stubs

#[derive(Args)]
pub struct PassArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<String>,
}

pub fn not_implemented(name: &str, when: &str) -> CliResult<()> {
    Err(CliError::new(
        GENERAL,
        format!("`stewdio {name}` is not implemented in v0.1 (planned for {when})"),
    ))
}
