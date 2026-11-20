//! `stewdio rojo export|import`: convert between Stewdio projects and Rojo projects.
//!
//! Rojo layout produced by `export`: `default.project.json` plus `src/<Service>/...` where scripts are
//! `.server.lua` / `.client.lua` / `.lua` files (with `init.*` for scripts that have children),
//! folders are directories, and every other instance is a `.model.json` file.
use clap::{Args, Subcommand};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::*;
use crate::hjson::{self, Value};
use crate::model::*;
use crate::project::{self, Project};
use crate::{api, assets, commands, fsproj, ui, util};

#[derive(Args)]
pub struct RojoArgs {
    #[command(subcommand)]
    pub cmd: RojoCmd,
}

#[derive(Subcommand)]
pub enum RojoCmd {
    /// Convert the current Stewdio project into a Rojo project
    Export {
        /// Output directory (default: <project>-rojo next to the project)
        #[arg(long)]
        output: Option<PathBuf>,
        /// Replace file:// asset references with rbxassetid:// ids from Assets.toml
        #[arg(long)]
        resolve_assets: bool,
        /// Allow replacing an existing output directory
        #[arg(long)]
        overwrite: bool,
    },
    /// Convert a Rojo project (directory or *.project.json) into a Stewdio project
    Import {
        /// Rojo project directory or project file
        input: PathBuf,
        /// Output directory (default: ./<project name>)
        #[arg(long)]
        output: Option<PathBuf>,
        /// Project name (default: the name in the Rojo project)
        #[arg(long)]
        name: Option<String>,
        /// Allow replacing an existing output directory
        #[arg(long)]
        overwrite: bool,
    },
}

pub fn run(a: RojoArgs) -> CliResult<()> {
    match a.cmd {
        RojoCmd::Export { output, resolve_assets, overwrite } => export(output, resolve_assets, overwrite),
        RojoCmd::Import { input, output, name, overwrite } => import(input, output, name, overwrite),
    }
}

// ---------------------------------------------------------------- JSON helpers

fn jstr(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn jnum(t: &str) -> Value {
    let t = t.trim();
    if crate::util::is_json_number(t) {
        Value::Number(t.to_string())
    } else {
        Value::Number("0".to_string())
    }
}

fn one(k: &str, v: Value) -> Value {
    Value::Object(vec![(k.to_string(), v)])
}

fn obj(items: Vec<(&str, Value)>) -> Value {
    Value::Object(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn text_of(v: &PropValue) -> &str {
    match v {
        PropValue::Text(t) => t.as_str(),
        _ => "",
    }
}

fn field_of<'a>(v: &'a PropValue, k: &str) -> &'a str {
    if let PropValue::Fields(f) = v {
        for (n, x) in f {
            if n.as_str() == k {
                if let PropValue::Text(t) = x {
                    return t.as_str();
                }
            }
        }
    }
    "0"
}

fn get_ci<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    for k in keys {
        if let Some(x) = v.get(k) {
            return Some(x);
        }
    }
    None
}

fn write_json(path: &Path, v: &Value) -> io::Result<()> {
    fs::write(path, hjson::to_json_string(v))
}

// ---------------------------------------------------------------------- export

struct Exporter {
    warnings: Vec<String>,
}

const RESERVED: &[&str] = &["init.meta.json", "init.server.lua", "init.client.lua", "init.lua", "default.project.json"];

impl Exporter {
    fn prop(&mut self, path: &str, p: &Property) -> Option<(String, Value)> {
        let name = match p.name.as_str() {
            "size" => "Size",
            "Color3uint8" => "Color",
            n => n,
        }
        .to_string();
        let t = text_of(&p.value);
        let v = &p.value;
        let value = match p.ty.as_str() {
            "bool" => Value::Bool(t.trim() == "true"),
            "string" | "ProtectedString" => Value::Str(t.to_string()),
            "int" | "int64" | "float" | "double" => jnum(t),
            "token" => one("Enum", jnum(t)),
            "BrickColor" => one("BrickColor", jnum(t)),
            "Vector3" => one(
                "Vector3",
                Value::Array(vec![jnum(field_of(v, "X")), jnum(field_of(v, "Y")), jnum(field_of(v, "Z"))]),
            ),
            "Vector2" => one("Vector2", Value::Array(vec![jnum(field_of(v, "X")), jnum(field_of(v, "Y"))])),
            "Color3" => one(
                "Color3",
                Value::Array(vec![jnum(field_of(v, "R")), jnum(field_of(v, "G")), jnum(field_of(v, "B"))]),
            ),
            "Color3uint8" => {
                let u: u32 = t.trim().parse().unwrap_or(0);
                let c = |sh: u32| jnum(&format!("{}", ((u >> sh) & 255) as f32 / 255.0));
                one("Color3", Value::Array(vec![c(16), c(8), c(0)]))
            }
            "UDim" => one("UDim", Value::Array(vec![jnum(field_of(v, "S")), jnum(field_of(v, "O"))])),
            "UDim2" => one(
                "UDim2",
                Value::Array(vec![
                    Value::Array(vec![jnum(field_of(v, "XS")), jnum(field_of(v, "XO"))]),
                    Value::Array(vec![jnum(field_of(v, "YS")), jnum(field_of(v, "YO"))]),
                ]),
            ),
            "CoordinateFrame" => {
                let m = |a: &str, b: &str, c: &str| Value::Array(vec![jnum(field_of(v, a)), jnum(field_of(v, b)), jnum(field_of(v, c))]);
                one(
                    "CFrame",
                    obj(vec![
                        ("position", m("X", "Y", "Z")),
                        ("orientation", Value::Array(vec![m("R00", "R01", "R02"), m("R10", "R11", "R12"), m("R20", "R21", "R22")])),
                    ]),
                )
            }
            "Content" => {
                let url = match v {
                    PropValue::Text(t) => t.clone(),
                    PropValue::Fields(_) => {
                        let u = field_of(v, "url");
                        if u == "0" { String::new() } else { u.to_string() }
                    }
                };
                if url.is_empty() {
                    return None;
                }
                one("Content", Value::Str(url))
            }
            "NumberRange" => {
                let mut it = t.split_whitespace();
                let a = it.next().unwrap_or("0");
                let b = it.next().unwrap_or("0");
                one("NumberRange", Value::Array(vec![jnum(a), jnum(b)]))
            }
            other => {
                self.warnings.push(format!("{path}: property '{}' of type {other} has no Rojo equivalent and was skipped", p.name));
                return None;
            }
        };
        Some((name, value))
    }

    fn props(&mut self, path: &str, inst: &Instance) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for p in &inst.properties {
            if let Some(kv) = self.prop(path, p) {
                out.push(kv);
            }
        }
        if !inst.tags.is_empty() {
            self.warnings.push(format!("{path}: tags are not exported to Rojo"));
        }
        out
    }

    fn model_json(&mut self, path: &str, inst: &Instance, root: bool) -> Value {
        let mut o: Vec<(&str, Value)> = Vec::new();
        if !root {
            o.push(("Name", jstr(&inst.name)));
        }
        o.push(("ClassName", jstr(&inst.class)));
        let mut props = self.props(path, inst);
        if let Some(src) = &inst.source {
            props.insert(0, ("Source".to_string(), Value::Str(src.clone())));
        }
        if !props.is_empty() {
            o.push(("Properties", Value::Object(props)));
        }
        if !inst.children.is_empty() {
            let mut kids = Vec::new();
            for c in &inst.children {
                let cp = format!("{path}/{}", c.name);
                kids.push(self.model_json(&cp, c, false));
            }
            o.push(("Children", Value::Array(kids)));
        }
        obj(o)
    }

    fn children(&mut self, dir: &Path, list: &[Instance], path: &str) -> io::Result<()> {
        let mut used: HashSet<String> = RESERVED.iter().map(|s| s.to_string()).collect();
        for c in list {
            self.node(dir, c, &mut used, path)?;
        }
        Ok(())
    }

    /// Write one instance below `dir`; returns the file or directory name that was created.
    fn node(&mut self, dir: &Path, inst: &Instance, used: &mut HashSet<String>, parent: &str) -> io::Result<String> {
        let path = if parent.is_empty() { inst.name.clone() } else { format!("{parent}/{}", inst.name) };
        let base = util::sanitize_name(&inst.name);
        if let Some(kind) = script_kind(&inst.class) {
            let ext = match kind {
                "server" => "server.lua",
                "client" => "client.lua",
                _ => "lua",
            };
            let src = inst.source.clone().unwrap_or_default();
            let props = self.props(&path, inst);
            if inst.children.is_empty() {
                let (b, fname) = fsproj::unique(used, &base, |b: &str| format!("{b}.{ext}"));
                fs::write(dir.join(&fname), src)?;
                if !props.is_empty() {
                    write_json(&dir.join(format!("{b}.meta.json")), &obj(vec![("properties", Value::Object(props))]))?;
                }
                return Ok(fname);
            }
            let (_, dname) = fsproj::unique(used, &base, |b: &str| b.to_string());
            let d = dir.join(&dname);
            fs::create_dir_all(&d)?;
            fs::write(d.join(format!("init.{ext}")), src)?;
            if !props.is_empty() {
                write_json(&d.join("init.meta.json"), &obj(vec![("properties", Value::Object(props))]))?;
            }
            self.children(&d, &inst.children, &path)?;
            return Ok(dname);
        }
        if inst.class == "Folder" {
            let (_, dname) = fsproj::unique(used, &base, |b: &str| b.to_string());
            let d = dir.join(&dname);
            fs::create_dir_all(&d)?;
            let props = self.props(&path, inst);
            if !props.is_empty() {
                write_json(&d.join("init.meta.json"), &obj(vec![("properties", Value::Object(props))]))?;
            }
            if inst.children.is_empty() {
                fs::write(d.join(".gitkeep"), "")?;
            }
            self.children(&d, &inst.children, &path)?;
            return Ok(dname);
        }
        let (_, fname) = fsproj::unique(used, &base, |b: &str| format!("{b}.model.json"));
        let v = self.model_json(&path, inst, true);
        write_json(&dir.join(&fname), &v)?;
        Ok(fname)
    }
}

fn export(output: Option<PathBuf>, resolve: bool, overwrite: bool) -> CliResult<()> {
    let project = Project::current()?;
    let is_model = project.is_model();
    let loaded = fsproj::load(&project.root, is_model);
    if let Some(e) = loaded.diags.iter().find(|d| d.is_error) {
        return Err(CliError::new(PROJECT, e.msg.clone()));
    }
    let mut roots = loaded.roots;
    if resolve {
        let known = assets::known_files(&project.root);
        let reg = assets::Registry::load(&project.root);
        let ids: HashMap<String, u64> = reg.entries.iter().map(|(k, e)| (k.clone(), e.id)).collect();
        assets::rewrite(&mut roots, &known, &ids);
    }
    let name = project.file.project.name.clone();
    let out = output.unwrap_or_else(|| {
        let dir = project.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
        project.root.with_file_name(format!("{dir}-rojo"))
    });
    commands::prepare_output_dir(&out, overwrite)?;
    let src = out.join("src");
    fs::create_dir_all(&src)?;
    let mut ex = Exporter { warnings: Vec::new() };

    let tree = if !is_model {
        let mut node: Vec<(String, Value)> = vec![("$className".to_string(), jstr("DataModel"))];
        for svc in &roots {
            let dname = util::sanitize_name(&svc.name);
            let d = src.join(&dname);
            fs::create_dir_all(&d)?;
            let props = ex.props(&svc.name, svc);
            if !props.is_empty() {
                write_json(&d.join("init.meta.json"), &obj(vec![("properties", Value::Object(props))]))?;
            }
            if svc.children.is_empty() {
                fs::write(d.join(".gitkeep"), "")?;
            }
            ex.children(&d, &svc.children, &svc.name)?;
            let mut entry = Vec::new();
            if svc.class != svc.name {
                entry.push(("$className", jstr(&svc.class)));
            }
            entry.push(("$path", Value::Str(format!("src/{dname}"))));
            node.push((svc.name.clone(), obj(entry)));
        }
        Value::Object(node)
    } else if roots.len() == 1 {
        let mut used = HashSet::new();
        let created = ex.node(&src, &roots[0], &mut used, "")?;
        obj(vec![("$path", Value::Str(format!("src/{created}")))])
    } else {
        ex.children(&src, &roots, "")?;
        obj(vec![("$className", jstr("Folder")), ("$path", jstr("src"))])
    };
    write_json(&out.join("default.project.json"), &obj(vec![("name", Value::Str(name)), ("tree", tree)]))?;

    ui::success("\nExported to a Rojo project!\n");
    println!("Output: {}", out.display());
    println!("Warnings: {}", ex.warnings.len());
    for w in ex.warnings.iter().take(10) {
        ui::warning(w);
    }
    if ex.warnings.len() > 10 {
        println!("... and {} more", ex.warnings.len() - 10);
    }
    println!("\nNext steps:\n  cd {}\n  rojo build -o game.rbxl", out.display());
    if !resolve && assets::collect(&roots, &assets::known_files(&project.root)).refs.len() > 0 {
        ui::hint("file:// asset references were left as they are; use --resolve-assets to replace them with uploaded asset ids");
    }
    Ok(())
}

// ---------------------------------------------------------------------- import

#[derive(PartialEq, Clone, Copy, Debug)]
enum Kind {
    Script(&'static str),
    Model,
    Meta,
    Rbx,
    Txt,
    Project,
    Other,
}

fn classify(fname: &str) -> (String, Kind) {
    let lower = fname.to_ascii_lowercase();
    let cut = |suf: &str| fname[..fname.len() - suf.len()].to_string();
    for (suf, class) in [
        (".server.lua", "Script"),
        (".server.luau", "Script"),
        (".client.lua", "LocalScript"),
        (".client.luau", "LocalScript"),
    ] {
        if lower.ends_with(suf) {
            return (cut(suf), Kind::Script(class));
        }
    }
    for (suf, kind) in [
        (".model.json", Kind::Model),
        (".meta.json", Kind::Meta),
        (".project.json", Kind::Project),
        (".rbxmx", Kind::Rbx),
        (".rbxm", Kind::Rbx),
        (".luau", Kind::Script("ModuleScript")),
        (".lua", Kind::Script("ModuleScript")),
        (".txt", Kind::Txt),
    ] {
        if lower.ends_with(suf) {
            return (cut(suf), kind);
        }
    }
    (fname.to_string(), Kind::Other)
}

fn nums(x: &Value, n: usize) -> Option<Vec<String>> {
    if let Value::Array(a) = x {
        let v: Option<Vec<String>> = a
            .iter()
            .map(|e| if let Value::Number(s) = e { Some(s.clone()) } else { None })
            .collect();
        v.filter(|v| v.len() == n)
    } else {
        None
    }
}

fn pv_fields(name: &str, ty: &str, items: Vec<(&str, String)>) -> Property {
    Property {
        name: name.to_string(),
        ty: ty.to_string(),
        value: PropValue::Fields(items.into_iter().map(|(k, v)| (k.to_string(), PropValue::Text(v))).collect()),
    }
}

fn pv_text(name: &str, ty: &str, t: String) -> Property {
    Property { name: name.to_string(), ty: ty.to_string(), value: PropValue::Text(t) }
}

fn clean_f(x: f64) -> String {
    let r = (x * 1e6).round() / 1e6;
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r}")
}

fn mat_mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                o[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    o
}

fn cframe(name: &str, v: &Value) -> Result<Property, String> {
    let pos = v.get("position").and_then(|p| nums(p, 3)).ok_or("CFrame position must be [x, y, z]")?;
    let rot: Vec<String> = match v.get("orientation") {
        None => ["1", "0", "0", "0", "1", "0", "0", "0", "1"].iter().map(|s| s.to_string()).collect(),
        Some(Value::Array(rows)) if rows.len() == 3 && rows.iter().all(|r| matches!(r, Value::Array(_))) => {
            let mut out = Vec::new();
            for r in rows {
                out.extend(nums(r, 3).ok_or("CFrame orientation rows must be [a, b, c]")?);
            }
            out
        }
        Some(o) => {
            let e = nums(o, 3).ok_or("CFrame orientation must be 3 angles or a 3x3 matrix")?;
            let ang: Vec<f64> = e.iter().map(|s| s.parse::<f64>().unwrap_or(0.0).to_radians()).collect();
            let (rx, ry, rz) = (ang[0], ang[1], ang[2]);
            let mx = [[1.0, 0.0, 0.0], [0.0, rx.cos(), -rx.sin()], [0.0, rx.sin(), rx.cos()]];
            let my = [[ry.cos(), 0.0, ry.sin()], [0.0, 1.0, 0.0], [-ry.sin(), 0.0, ry.cos()]];
            let mz = [[rz.cos(), -rz.sin(), 0.0], [rz.sin(), rz.cos(), 0.0], [0.0, 0.0, 1.0]];
            let m = mat_mul(mat_mul(my, mx), mz);
            m.iter().flat_map(|r| r.iter()).map(|x| clean_f(*x)).collect()
        }
    };
    let keys = ["R00", "R01", "R02", "R10", "R11", "R12", "R20", "R21", "R22"];
    let mut items: Vec<(&str, String)> = vec![("X", pos[0].clone()), ("Y", pos[1].clone()), ("Z", pos[2].clone())];
    for (k, r) in keys.iter().zip(rot.into_iter()) {
        items.push((*k, r));
    }
    Ok(pv_fields(name, "CoordinateFrame", items))
}

fn unambiguous(name: &str, k: &str, x: &Value) -> Option<Result<Property, String>> {
    let bad = |what: &str| Err(format!("property '{name}': {k} expects {what}"));
    Some(match k {
        "Bool" => match x {
            Value::Bool(b) => Ok(pv_text(name, "bool", b.to_string())),
            _ => bad("true or false"),
        },
        "String" => match x {
            Value::Str(s) => Ok(pv_text(name, "string", s.clone())),
            _ => bad("a string"),
        },
        "Int32" | "Int64" | "Float32" | "Float64" | "Enum" | "BrickColor" => match x {
            Value::Number(n) => {
                let ty = match k {
                    "Int32" => "int",
                    "Int64" => "int64",
                    "Float32" => "float",
                    "Float64" => "double",
                    "Enum" => "token",
                    _ => "BrickColor",
                };
                Ok(pv_text(name, ty, n.clone()))
            }
            _ => bad("a number"),
        },
        "Content" | "ContentId" => match x {
            Value::Str(s) => Ok(pv_fields(name, "Content", vec![("url", s.clone())])),
            _ => bad("a string"),
        },
        "Vector3" => match nums(x, 3) {
            Some(n) => Ok(pv_fields(name, "Vector3", vec![("X", n[0].clone()), ("Y", n[1].clone()), ("Z", n[2].clone())])),
            None => bad("[x, y, z]"),
        },
        "Vector2" => match nums(x, 2) {
            Some(n) => Ok(pv_fields(name, "Vector2", vec![("X", n[0].clone()), ("Y", n[1].clone())])),
            None => bad("[x, y]"),
        },
        "Color3" => match nums(x, 3) {
            Some(n) => Ok(pv_fields(name, "Color3", vec![("R", n[0].clone()), ("G", n[1].clone()), ("B", n[2].clone())])),
            None => bad("[r, g, b]"),
        },
        "UDim" => match nums(x, 2) {
            Some(n) => Ok(pv_fields(name, "UDim", vec![("S", n[0].clone()), ("O", n[1].clone())])),
            None => bad("[scale, offset]"),
        },
        "UDim2" => {
            if let Value::Array(rows) = x {
                if rows.len() == 2 {
                    if let (Some(a), Some(b)) = (nums(&rows[0], 2), nums(&rows[1], 2)) {
                        return Some(Ok(pv_fields(
                            name,
                            "UDim2",
                            vec![("XS", a[0].clone()), ("XO", a[1].clone()), ("YS", b[0].clone()), ("YO", b[1].clone())],
                        )));
                    }
                }
            }
            bad("[[xs, xo], [ys, yo]]")
        }
        "NumberRange" => match nums(x, 2) {
            Some(n) => Ok(pv_text(name, "NumberRange", format!("{} {} ", n[0], n[1]))),
            None => bad("[min, max]"),
        },
        "CFrame" => cframe(name, x),
        _ => return None,
    })
}

fn rojo_value(name: &str, v: &Value) -> Result<Property, String> {
    if let Value::Object(o) = v {
        if o.len() == 1 {
            let (k, x) = &o[0];
            if let Some(r) = unambiguous(name, k, x) {
                return r;
            }
        }
        if v.get("position").is_some() {
            return cframe(name, v);
        }
    }
    hjson_to_prop(name, v)
}

fn collect_map(inst: &Instance, map: &mut HashMap<String, String>, next: &mut usize) {
    *next += 1;
    map.insert(inst.id.clone(), format!("instance-{:03}", *next));
    for c in &inst.children {
        collect_map(c, map, next);
    }
}

fn apply_map(inst: &mut Instance, map: &HashMap<String, String>) {
    if let Some(n) = map.get(&inst.id) {
        inst.id = n.clone();
    }
    for p in inst.properties.iter_mut() {
        if p.ty == "Ref" {
            if let PropValue::Text(t) = &mut p.value {
                if let Some(n) = map.get(t.as_str()) {
                    *t = n.clone();
                }
            }
        }
    }
    for c in inst.children.iter_mut() {
        apply_map(c, map);
    }
}

struct Importer {
    base: PathBuf,
    next: usize,
    warnings: Vec<String>,
}

impl Importer {
    fn inst(&mut self, class: &str, name: &str) -> Instance {
        self.next += 1;
        Instance::new(&format!("instance-{:03}", self.next), class, name)
    }

    fn read_json(&mut self, path: &Path) -> Option<Value> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.warnings.push(format!("{}: cannot read ({e})", path.display()));
                return None;
            }
        };
        match hjson::parse(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                self.warnings.push(format!("{}: invalid JSON ({e})", path.display()));
                None
            }
        }
    }

    fn add_props(&mut self, inst: &mut Instance, props: &Value, ctx: &str) {
        if let Value::Object(o) = props {
            for (k, v) in o {
                if k == "Tags" {
                    if let Value::Array(a) = v {
                        inst.tags = a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
                    }
                    continue;
                }
                match rojo_value(k, v) {
                    Ok(p) => absorb(inst, p),
                    Err(e) => self.warnings.push(format!("{ctx}: {e}")),
                }
            }
        }
    }

    fn apply_meta(&mut self, inst: &mut Instance, path: &Path, ctx: &str) {
        if let Some(v) = self.read_json(path) {
            if let Some(c) = get_ci(&v, &["className", "ClassName"]).and_then(|c| c.as_str()) {
                inst.class = c.to_string();
            }
            if let Some(p) = get_ci(&v, &["properties", "Properties"]) {
                self.add_props(inst, p, ctx);
            }
        }
    }

    fn model_value(&mut self, default_name: &str, v: &Value, ctx: &str) -> Option<Instance> {
        let class = get_ci(v, &["ClassName", "className"]).and_then(|c| c.as_str());
        let class = match class {
            Some(c) => c.to_string(),
            None => {
                self.warnings.push(format!("{ctx}: model entry without a ClassName was skipped"));
                return None;
            }
        };
        let name = get_ci(v, &["Name", "name"]).and_then(|n| n.as_str()).unwrap_or(default_name).to_string();
        let mut inst = self.inst(&class, &name);
        if let Some(p) = get_ci(v, &["Properties", "properties"]) {
            self.add_props(&mut inst, p, ctx);
        }
        if let Some(Value::Array(kids)) = get_ci(v, &["Children", "children"]) {
            for k in kids {
                let child_ctx = format!("{ctx}/{}", name);
                if let Some(c) = self.model_value(&class, k, &child_ctx) {
                    inst.children.push(c);
                }
            }
        }
        Some(inst)
    }

    fn script_file(&mut self, name: &str, path: &Path, class: &str) -> Option<Instance> {
        let src = match fs::read(path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                self.warnings.push(format!("{}: cannot read ({e})", path.display()));
                return None;
            }
        };
        let mut inst = self.inst(class, name);
        inst.source = Some(src);
        let fname = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (stem, _) = classify(&fname);
        let meta = path.with_file_name(format!("{stem}.meta.json"));
        if meta.is_file() {
            self.apply_meta(&mut inst, &meta, name);
        }
        Some(inst)
    }

    fn rbx_file(&mut self, name: &str, path: &Path) -> Option<Instance> {
        let data = fs::read(path).ok()?;
        let decoded = match commands::decode_bytes(&data) {
            Ok(d) => d,
            Err(e) => {
                self.warnings.push(format!("{}: {}", path.display(), e.message));
                return None;
            }
        };
        let mut roots = decoded.roots;
        if roots.is_empty() {
            return None;
        }
        if roots.len() > 1 {
            self.warnings.push(format!("{}: only the first of {} root instances was imported", path.display(), roots.len()));
        }
        let mut inst = roots.remove(0);
        let mut map = HashMap::new();
        collect_map(&inst, &mut map, &mut self.next);
        apply_map(&mut inst, &map);
        inst.name = name.to_string();
        Some(inst)
    }

    fn dir_instance(&mut self, name: &str, dir: &Path, ctx: &str) -> Option<Instance> {
        let mut inst = self.inst("Folder", name);
        let mut entries: Vec<(String, PathBuf)> = match fs::read_dir(dir) {
            Ok(rd) => rd.flatten().map(|e| (e.file_name().to_string_lossy().to_string(), e.path())).collect(),
            Err(e) => {
                self.warnings.push(format!("{}: cannot read directory ({e})", dir.display()));
                return None;
            }
        };
        entries.sort();
        // init script / init.meta.json describe the directory itself
        for (fname, p) in &entries {
            let (stem, kind) = classify(fname);
            if stem == "init" {
                if let Kind::Script(class) = kind {
                    if let Ok(b) = fs::read(p) {
                        inst.class = class.to_string();
                        inst.source = Some(String::from_utf8_lossy(&b).into_owned());
                    }
                }
            }
        }
        let init_meta = dir.join("init.meta.json");
        if init_meta.is_file() {
            self.apply_meta(&mut inst, &init_meta, ctx);
        }
        for (fname, p) in entries {
            if fname.starts_with('.') {
                continue;
            }
            let child_ctx = format!("{ctx}/{fname}");
            if p.is_dir() {
                if let Some(c) = self.dir_instance(&fname, &p, &child_ctx) {
                    inst.children.push(c);
                }
                continue;
            }
            let (stem, kind) = classify(&fname);
            if stem == "init" {
                continue;
            }
            let child = match kind {
                Kind::Script(class) => self.script_file(&stem, &p, class),
                Kind::Model => self.read_json(&p).and_then(|v| self.model_value(&stem, &v, &child_ctx)),
                Kind::Rbx => self.rbx_file(&stem, &p),
                Kind::Txt => {
                    let text = fs::read_to_string(&p).unwrap_or_default();
                    let mut sv = self.inst("StringValue", &stem);
                    sv.properties.push(pv_text("Value", "string", text));
                    Some(sv)
                }
                Kind::Meta => None,
                Kind::Project => {
                    self.warnings.push(format!("{child_ctx}: nested project files are not imported"));
                    None
                }
                Kind::Other => {
                    self.warnings.push(format!("{child_ctx}: unsupported file type was skipped"));
                    None
                }
            };
            if let Some(c) = child {
                inst.children.push(c);
            }
        }
        Some(inst)
    }

    fn path_instance(&mut self, name: &str, path: &Path, ctx: &str) -> Option<Instance> {
        if path.is_dir() {
            return self.dir_instance(name, path, ctx);
        }
        if !path.is_file() {
            self.warnings.push(format!("{ctx}: $path '{}' does not exist", path.display()));
            return None;
        }
        let fname = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (_, kind) = classify(&fname);
        match kind {
            Kind::Script(class) => self.script_file(name, path, class),
            Kind::Model => self.read_json(path).and_then(|v| self.model_value(name, &v, ctx)),
            Kind::Rbx => self.rbx_file(name, path),
            _ => {
                self.warnings.push(format!("{ctx}: unsupported $path file '{fname}'"));
                None
            }
        }
    }

    fn node(&mut self, key: &str, v: &Value, parent_is_datamodel: bool, ctx: &str) -> Option<Instance> {
        let entries = match v {
            Value::Object(o) => o,
            _ => {
                self.warnings.push(format!("{ctx}: project node must be an object"));
                return None;
            }
        };
        let class_override = v.get("$className").and_then(|c| c.as_str()).map(|s| s.to_string());
        let mut inst = match v.get("$path").and_then(|p| p.as_str()) {
            Some(p) => {
                let full = self.base.join(p);
                let mut i = self.path_instance(key, &full, ctx)?;
                if let Some(c) = &class_override {
                    i.class = c.clone();
                }
                i
            }
            None => {
                let class = class_override.unwrap_or_else(|| if parent_is_datamodel { key.to_string() } else { "Folder".to_string() });
                self.inst(&class, key)
            }
        };
        if let Some(p) = v.get("$properties") {
            self.add_props(&mut inst, p, ctx);
        }
        if v.get("$attributes").is_some() {
            self.warnings.push(format!("{ctx}: $attributes are not imported"));
        }
        for (k, child) in entries {
            if k.starts_with('$') {
                continue;
            }
            let cctx = format!("{ctx}/{k}");
            if let Some(c) = self.node(k, child, false, &cctx) {
                inst.children.push(c);
            }
        }
        Some(inst)
    }
}

fn import(input: PathBuf, output: Option<PathBuf>, name: Option<String>, overwrite: bool) -> CliResult<()> {
    let file = if input.is_dir() { input.join("default.project.json") } else { input.clone() };
    if !file.is_file() {
        return Err(CliError::new(ARGS, format!("Rojo project file '{}' not found", file.display())));
    }
    let text = fs::read_to_string(&file)?;
    let v = hjson::parse(&text).map_err(|e| CliError::new(PARSE, format!("Invalid Rojo project: {e}")))?;
    let tree = v
        .get("tree")
        .ok_or_else(|| CliError::new(PARSE, "The Rojo project has no \"tree\""))?;
    let base = file.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let name = name
        .or_else(|| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "rojo-project".to_string());
    let out = output.unwrap_or_else(|| PathBuf::from(".").join(util::sanitize_name(&name)));
    commands::prepare_output_dir(&out, overwrite)?;

    let is_place = tree.get("$className").and_then(|c| c.as_str()) == Some("DataModel");
    let mut imp = Importer { base, next: 0, warnings: Vec::new() };
    let mut roots: Vec<Instance> = Vec::new();
    if is_place {
        if let Value::Object(o) = tree {
            for (k, child) in o {
                if k.starts_with('$') {
                    continue;
                }
                if let Some(i) = imp.node(k, child, true, k) {
                    roots.push(i);
                }
            }
        }
    } else if let Some(i) = imp.node(&name, tree, false, &name) {
        roots.push(i);
    }

    // Rojo writes values loosely (enum names, [r,g,b], strings for assets); the API database types them.
    let database = api::Api::load();
    if database.is_none() {
        imp.warnings.push("Roblox API data is not installed: enum names and similar values stay untyped. Run `stewdio api update` and import again".to_string());
    }
    let fill = api::process(database.as_ref(), &mut roots, api::Mode::Normalize);
    imp.warnings.extend(fill.errors);
    imp.warnings.extend(fill.warnings);

    fs::create_dir_all(&out)?;
    let mut write_warnings = Vec::new();
    fsproj::write_children(&out, &roots, &fsproj::WriteOptions { write_scripts: true }, &mut write_warnings)?;
    let kind = if is_place { "place" } else { "model" };
    fs::write(out.join("Project.toml"), project::render_project_toml(&name, kind, "baseplate"))?;
    fs::write(out.join(".stewdioignore"), project::DEFAULT_IGNORE)?;
    fs::create_dir_all(out.join(assets::DIR))?;
    fs::write(out.join(assets::DIR).join(".gitkeep"), "")?;
    imp.warnings.extend(write_warnings);

    let instances: usize = roots.iter().map(|r| r.count()).sum();
    let scripts: usize = roots.iter().map(|r| r.script_count()).sum();
    ui::success("\nImported the Rojo project!\n");
    println!("Name: {name}");
    println!("Type: {kind}");
    println!("Instances: {instances}");
    println!("Scripts: {scripts}");
    println!("Warnings: {}", imp.warnings.len());
    for w in imp.warnings.iter().take(10) {
        ui::warning(w);
    }
    if imp.warnings.len() > 10 {
        println!("... and {} more", imp.warnings.len() - 10);
    }
    println!("Output: {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_rojo_files() {
        assert_eq!(classify("Main.server.lua"), ("Main".to_string(), Kind::Script("Script")));
        assert_eq!(classify("Hud.client.luau"), ("Hud".to_string(), Kind::Script("LocalScript")));
        assert_eq!(classify("Util.lua"), ("Util".to_string(), Kind::Script("ModuleScript")));
        assert_eq!(classify("Thing.model.json").1, Kind::Model);
        assert_eq!(classify("README.md").1, Kind::Other);
    }

    #[test]
    fn converts_unambiguous_values() {
        let v = hjson::parse("{ \"Vector3\": [1, 2, 3] }").unwrap();
        let p = rojo_value("Size", &v).unwrap();
        assert_eq!(p.ty, "Vector3");
        let v = hjson::parse("{ \"position\": [0, 5, 0], \"orientation\": [0, 90, 0] }").unwrap();
        let p = rojo_value("CFrame", &v).unwrap();
        assert_eq!(p.ty, "CoordinateFrame");
    }
}
