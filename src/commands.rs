use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::*;
use crate::fsproj::{self, WriteOptions};
use crate::model::*;
use crate::project::{self, xml_from_ext, Project};
use crate::{api, rbxl, rbxlx, ui, util, validate};

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
    let model = a.model;
    let template = if a.empty {
        "empty".to_string()
    } else {
        a.template.clone().unwrap_or_else(|| {
            crate::config::Global::load().get("templates.name").unwrap_or_else(|| "baseplate".to_string())
        })
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

    let roots = if model {
        if template == "empty" {
            Vec::new()
        } else {
            vec![Instance::new("instance-001", "Model", &project_name)]
        }
    } else {
        default_tree(template == "baseplate")
    };
    let mut warnings = Vec::new();
    fsproj::write_children(&dir, &roots, &WriteOptions { write_scripts: true }, &mut warnings)?;
    let kind = if model { "model" } else { "place" };
    fs::write(dir.join("Project.toml"), project::render_project_toml(&project_name, kind, &template))?;
    fs::write(dir.join(".stewdioignore"), project::DEFAULT_IGNORE)?;

    println!("Created {kind} project '{}' at {}", project_name, dir.display());
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
    let cwd = std::env::current_dir()?;
    let name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "stewdio-project".to_string());

    let r1 = write_guarded(&cwd.join("Project.toml"), &project::render_project_toml(&name, if a.model { "model" } else { "place" }, "baseplate"), a.force)?;
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
        } else if !a.model && SERVICES.contains(&dname.as_str()) {
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
    /// Compile into XML format (.rbxlx / .rbxmx)
    #[arg(long, conflicts_with = "binary")]
    pub xml: bool,
    /// Compile into binary format (.rbxl / .rbxm)
    #[arg(long)]
    pub binary: bool,
    /// Specify the output file (the extension selects the format unless --xml/--binary is given)
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
    /// Disable compilation caching (there is no cache yet; accepted for compatibility)
    #[arg(long)]
    pub no_cache: bool,
    /// Property autofill mode: off, normalize or defaults
    #[arg(long)]
    pub autofill: Option<String>,
}

pub struct BuildResult {
    pub out: PathBuf,
    pub use_xml: bool,
    pub instances: usize,
    pub scripts: usize,
    pub warnings: usize,
}

/// Decide the output format and path. The file extension is trusted: `--output game.rbxlx`
/// always produces XML, never binary bytes with an XML name.
fn resolve_output(project: &Project, a: &CompileArgs) -> CliResult<(bool, PathBuf)> {
    let st = project.settings();
    let explicit = if a.xml {
        Some(true)
    } else if a.binary {
        Some(false)
    } else {
        None
    };
    if let (Some(x), Some(p)) = (explicit, &a.output) {
        if let Some(e) = xml_from_ext(p) {
            if e != x {
                return Err(CliError::new(
                    ARGS,
                    format!(
                        "Output file '{}' implies {} format but --{} was given",
                        p.display(),
                        if e { "XML" } else { "binary" },
                        if x { "xml" } else { "binary" }
                    ),
                ));
            }
        }
    }
    let toml_out = project.file.compiler.output.as_ref().map(|o| project.root.join(o));
    let use_xml = match explicit {
        Some(x) => x,
        None => a
            .output
            .as_ref()
            .and_then(|p| xml_from_ext(p))
            .or_else(|| toml_out.as_ref().and_then(|p| xml_from_ext(p)))
            .unwrap_or(st.format == "xml"),
    };
    let out = match &a.output {
        Some(p) => p.clone(),
        None => project.output_path(use_xml),
    };
    Ok((use_xml, out))
}

fn verify(bytes: &[u8], use_xml: bool, expected: usize) -> Result<(), String> {
    let decoded = if use_xml {
        let text = std::str::from_utf8(bytes).map_err(|_| "output is not valid UTF-8".to_string())?;
        rbxlx::decode(text)?
    } else {
        rbxl::decode(bytes)?
    };
    let n: usize = decoded.roots.iter().map(|r| r.count()).sum();
    if n != expected {
        return Err(format!("expected {expected} instances but the output contains {n}"));
    }
    Ok(())
}

pub fn build(project: &Project, a: &CompileArgs) -> CliResult<BuildResult> {
    let started = std::time::Instant::now();
    let q = a.quiet;
    let st = project.settings();
    let is_model = project.is_model();
    let (use_xml, out) = resolve_output(project, a)?;

    say(q, "[Stewdio] Loading project...");
    let loaded = fsproj::load(&project.root, is_model);
    say(q, "[Stewdio] Reading instance metadata...");
    say(q, "[Stewdio] Loading Luau scripts...");
    say(q, "[Stewdio] Resolving references...");
    let report = validate::validate(&loaded, is_model);
    let mut errors = report.errors.clone();
    let mut warnings = report.warnings.clone();

    let mode = a.autofill.as_deref().map(api::Mode::parse).unwrap_or(st.autofill);
    let database = if mode == api::Mode::Off { None } else { api::Api::load() };
    if mode != api::Mode::Off && !q {
        if let Some(h) = api::stale_hint(database.as_ref()) {
            ui::hint(&h);
        }
    }
    let mut roots = loaded.roots.clone();
    let fill = api::process(database.as_ref(), &mut roots, mode);
    errors.extend(fill.errors);
    warnings.extend(fill.warnings);

    let release_failures = if a.release { warnings.len() } else { 0 };
    if !errors.is_empty() || release_failures > 0 {
        for e in &errors {
            ui::error(e);
        }
        if a.release {
            for w in &warnings {
                ui::error(&format!("(release) {w}"));
            }
        }
        return Err(CliError::new(
            PROJECT,
            format!("Project validation failed with {} error(s)", errors.len() + release_failures),
        ));
    }

    say(q, "[Stewdio] Building DataModel...");
    let bytes: Vec<u8> = if use_xml {
        say(q, "[Stewdio] Serializing XML output...");
        rbxlx::encode(&roots).map_err(|e| CliError::new(COMPILE, e))?.into_bytes()
    } else {
        say(q, "[Stewdio] Serializing binary output...");
        let enc = rbxl::encode(&roots, !is_model, st.compression).map_err(|e| CliError::new(COMPILE, e))?;
        warnings.extend(enc.warnings);
        enc.bytes
    };
    let instances: usize = roots.iter().map(|r| r.count()).sum();
    let scripts: usize = roots.iter().map(|r| r.script_count()).sum();
    say(q, "[Stewdio] Verifying output...");
    verify(&bytes, use_xml, instances)
        .map_err(|e| CliError::new(COMPILE, format!("Output verification failed: {e}")))?;

    if a.verbose && !q {
        for w in &warnings {
            ui::warning(w);
        }
        println!("[Stewdio] Paths excluded by .stewdioignore: {}", loaded.ignored);
        if fill.filled > 0 {
            println!("[Stewdio] Autofilled {} default propert{}", fill.filled, if fill.filled == 1 { "y" } else { "ies" });
        }
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&out, bytes)?;
    if !q {
        ui::success("\nCompilation successful!\n");
        println!("Output: {}", out.display());
        println!("Format: {}", if use_xml { "XML" } else { "binary" });
        println!("Instances: {instances}");
        println!("Scripts: {scripts}");
        println!("Warnings: {}", warnings.len());
        println!("Time: {} ms", started.elapsed().as_millis());
    }
    Ok(BuildResult { out, use_xml, instances, scripts, warnings: warnings.len() })
}

pub fn compile(a: CompileArgs) -> CliResult<()> {
    let project = Project::current()?;
    build(&project, &a).map(|_| ())
}

// ----------------------------------------------------------- decompile

#[derive(Args)]
pub struct DecompileArgs {
    /// Roblox place or model file (.rbxl, .rbxlx, .rbxm, .rbxmx)
    pub input: PathBuf,
    /// Specify the output directory
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Export as a model project
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

/// Parse place/model bytes (binary or XML).
pub fn decode_bytes(data: &[u8]) -> CliResult<rbxlx::Decoded> {
    if data.starts_with(b"<roblox!") {
        return rbxl::decode(data).map_err(|e| CliError::new(PARSE, e));
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(64)]).to_string();
    let head = head.trim_start_matches('\u{feff}').trim_start().to_string();
    if !(head.starts_with("<roblox") || head.starts_with("<?xml")) {
        return Err(CliError::new(UNSUPPORTED, "Unsupported input file format"));
    }
    let text = std::str::from_utf8(data).map_err(|_| CliError::new(PARSE, "Input file is not valid UTF-8"))?;
    rbxlx::decode(text).map_err(|e| CliError::new(PARSE, e))
}

pub struct ImportStats {
    pub instances: usize,
    pub scripts: usize,
    pub warnings: Vec<String>,
}

/// Turn place/model bytes into a project directory.
pub fn import_bytes(
    data: &[u8],
    stem: &str,
    out: &Path,
    as_model: bool,
    input_is_model: bool,
    no_scripts: bool,
    universe: Option<u64>,
    place: Option<u64>,
) -> CliResult<ImportStats> {
    let decoded = decode_bytes(data)?;
    let mut warnings = decoded.warnings.clone();
    let mut roots = decoded.roots;
    if as_model && !input_is_model {
        let mut flat = Vec::new();
        let mut flattened = 0;
        for r in roots {
            if SERVICES.contains(&r.class.as_str()) {
                flattened += 1;
                flat.extend(r.children);
            } else {
                flat.push(r);
            }
        }
        roots = flat;
        if flattened > 0 {
            warnings.push(format!("{flattened} service(s) were flattened into model roots; service properties were dropped"));
        }
    }
    fs::create_dir_all(out)?;
    fsproj::write_children(out, &roots, &WriteOptions { write_scripts: !no_scripts }, &mut warnings)?;
    let kind = if as_model { "model" } else { "place" };
    fs::write(out.join("Project.toml"), project::render_project_toml_with_ids(stem, kind, "baseplate", universe, place))?;
    fs::write(out.join(".stewdioignore"), project::DEFAULT_IGNORE)?;
    Ok(ImportStats {
        instances: roots.iter().map(|r| r.count()).sum(),
        scripts: roots.iter().map(|r| r.script_count()).sum(),
        warnings,
    })
}

/// Make sure `out` can receive a project (optionally clearing it after confirmation).
pub fn prepare_output_dir(out: &Path, overwrite: bool) -> CliResult<()> {
    if out.is_file() {
        return Err(CliError::new(GENERAL, format!("Output path '{}' is a file", out.display())));
    }
    if dir_nonempty(out) {
        if !overwrite {
            return Err(CliError::new(
                GENERAL,
                format!("Output directory '{}' already exists and is not empty (use --overwrite)", out.display()),
            ));
        }
        let canon = fs::canonicalize(out)?;
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
        fs::remove_dir_all(out)?;
    }
    Ok(())
}

pub fn decompile(a: DecompileArgs) -> CliResult<()> {
    if !a.input.is_file() {
        return Err(CliError::new(ARGS, format!("Input file '{}' not found", a.input.display())));
    }
    println!("[Stewdio] Reading {}...", a.input.display());
    let data = fs::read(&a.input)?;
    let stem = a
        .input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    let out = a.output.clone().unwrap_or_else(|| PathBuf::from(".").join(util::sanitize_name(&stem)));
    prepare_output_dir(&out, a.overwrite)?;
    let input_is_model = matches!(
        a.input.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("rbxm") | Some("rbxmx")
    );
    println!(
        "[Stewdio] Detecting {} format...",
        if data.starts_with(b"<roblox!") { "binary" } else { "XML" }
    );
    println!("[Stewdio] Reconstructing instances, properties and scripts...");
    let stats = import_bytes(&data, &stem, &out, a.model || input_is_model, input_is_model, a.no_scripts, None, None)?;
    if a.verbose {
        for w in &stats.warnings {
            ui::warning(w);
        }
    }
    ui::success("\nDecompilation successful!\n");
    println!("Instances: {}", stats.instances);
    println!("Scripts: {}", stats.scripts);
    println!("Warnings: {}", stats.warnings.len());
    println!("Output: {}", out.display());
    Ok(())
}

// --------------------------------------------------------------- open

/// Compile the project and open the result with the system's default program (usually Roblox Studio).
pub fn open_built() -> CliResult<()> {
    let project = Project::current()?;
    let built = build(
        &project,
        &CompileArgs { xml: false, binary: false, output: None, release: false, verbose: false, quiet: true, no_cache: false, autofill: None },
    )?;
    ui::step(&format!("Opening {}", built.out.display()));
    let status = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&built.out).status()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&built.out).status()
    } else {
        std::process::Command::new("xdg-open").arg(&built.out).status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(CliError::new(GENERAL, format!("Could not open the file automatically; open {} yourself", built.out.display()))),
    }
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
    let is_model = project.is_model();
    let loaded = fsproj::load(&project.root, is_model);
    let report = validate::validate(&loaded, is_model);
    let mut errors = report.errors.clone();
    let mut warnings = report.warnings.clone();
    let st = project.settings();
    let database = api::Api::load();
    if st.autofill != api::Mode::Off {
        let mut tmp = loaded.roots.clone();
        let fill = api::process(database.as_ref(), &mut tmp, st.autofill);
        errors.extend(fill.errors);
        warnings.extend(fill.warnings);
    }
    if let Some(h) = api::stale_hint(database.as_ref()) {
        ui::hint(&h);
    }
    if a.verbose {
        println!("[Stewdio] Project.toml: OK");
        println!("[Stewdio] Checked instance metadata, references, property types, class names,");
        println!("[Stewdio] duplicate identifiers, filesystem structure and script files.");
        match &database {
            Some(_) => println!("[Stewdio] Roblox API database: used for class/property checks"),
            None => println!("[Stewdio] Roblox API database: not installed (run `stewdio api update`)"),
        }
        println!("[Stewdio] Paths excluded by .stewdioignore: {}", loaded.ignored);
    }
    for e in &errors {
        ui::error(e);
    }
    for w in &warnings {
        if a.strict {
            ui::error(w);
        } else {
            ui::warning(w);
        }
    }
    let failed = errors.len() + if a.strict { warnings.len() } else { 0 };
    if failed > 0 {
        return Err(CliError::new(PROJECT, format!("Validation failed with {failed} error(s)")));
    }
    ui::success(&format!("Validation passed ({} warning(s))", warnings.len()));
    Ok(())
}

// ---------------------------------------------------------------- info

pub fn info() -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root, project.is_model());
    let instances: usize = loaded.roots.iter().map(|r| r.count()).sum();
    let scripts: usize = loaded.roots.iter().map(|r| r.script_count()).sum();
    let opt = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "(not set)".to_string());
    println!("Project name:       {}", project.file.project.name);
    println!("Project version:    {}", project.file.project.version);
    println!("Project type:       {}", project.file.project.kind);
    println!("Stewdio format:     {}", project::FORMAT_VERSION);
    println!("Compile format:     {}", project.settings().format);
    println!("Universe ID:        {}", opt(project.file.roblox.universe_id));
    println!("Place ID:           {}", opt(project.file.roblox.place_id));
    println!("Instances:          {instances}");
    println!("Scripts:            {scripts}");
    println!("Project directory:  {}", project.root.display());
    let database = api::Api::load();
    println!("Roblox API data:    {}", match &database { Some(d) => format!("installed ({} day(s) old)", d.age_days().unwrap_or(0)), None => "not installed (stewdio api update)".to_string() });
    if let Some(h) = api::stale_hint(database.as_ref()) {
        ui::hint(&h);
    }
    Ok(())
}

