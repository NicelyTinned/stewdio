//! Filesystem project <-> Instance tree.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::hjson::{self, Value};
use crate::ignore::IgnoreSet;
use crate::model::*;
use crate::util;

const RESERVED: &[&str] = &[
    "project.toml",
    ".stewdioignore",
    "instance.data.hjson",
    "init.server.lua",
    "init.client.lua",
    "init.module.lua",
];

// ------------------------------------------------------------- writing

pub struct WriteOptions {
    pub write_scripts: bool,
}

pub fn meta_value(inst: &Instance) -> Value {
    let props: Vec<(String, Value)> =
        inst.properties.iter().map(|p| (p.name.clone(), prop_to_hjson(p))).collect();
    Value::Object(vec![
        ("$className".to_string(), Value::Str(inst.class.clone())),
        ("$name".to_string(), Value::Str(inst.name.clone())),
        ("$id".to_string(), Value::Str(inst.id.clone())),
        ("Properties".to_string(), Value::Object(props)),
        ("Attributes".to_string(), Value::Object(Vec::new())),
        ("Tags".to_string(), Value::Array(inst.tags.iter().map(|t| Value::Str(t.clone())).collect())),
    ])
}

pub fn write_meta(dir: &Path, inst: &Instance) -> io::Result<()> {
    fs::write(dir.join("Instance.data.hjson"), hjson::to_string(&meta_value(inst)))
}

fn unique(used: &mut HashSet<String>, base: &str, make: impl Fn(&str) -> String) -> (String, String) {
    let mut b = base.to_string();
    let mut n = 1;
    loop {
        let f = make(&b);
        let key = f.to_lowercase();
        if !used.contains(&key) {
            used.insert(key);
            return (b, f);
        }
        n += 1;
        b = format!("{base}~{n}");
    }
}

pub fn write_children(
    dir: &Path,
    children: &[Instance],
    opts: &WriteOptions,
    warnings: &mut Vec<String>,
) -> io::Result<()> {
    let mut used: HashSet<String> = RESERVED.iter().map(|s| s.to_string()).collect();
    for c in children {
        write_instance(dir, c, &mut used, opts, warnings)?;
    }
    Ok(())
}

fn write_instance(
    dir: &Path,
    inst: &Instance,
    used: &mut HashSet<String>,
    opts: &WriteOptions,
    warnings: &mut Vec<String>,
) -> io::Result<()> {
    let base = util::sanitize_name(&inst.name);
    let kind = script_kind(&inst.class);
    let as_file = opts.write_scripts && inst.source.is_some() && inst.children.is_empty() && kind.is_some();
    if as_file {
        let kind = kind.unwrap_or("module");
        let (b, fname) = unique(used, &base, |b: &str| format!("{b}.{kind}.lua"));
        if b != base {
            warnings.push(format!("'{}' was renamed to '{}' on disk (name collision)", inst.name, b));
        }
        let stem = format!("{b}.{kind}");
        fs::write(dir.join(&fname), inst.source.as_deref().unwrap_or(""))?;
        fs::write(dir.join(format!("{stem}.data.hjson")), hjson::to_string(&meta_value(inst)))?;
        return Ok(());
    }
    let (b, dname) = unique(used, &base, |b: &str| b.to_string());
    if b != base {
        warnings.push(format!("'{}' was renamed to '{}' on disk (name collision)", inst.name, b));
    }
    let d = dir.join(&dname);
    fs::create_dir_all(&d)?;
    write_meta(&d, inst)?;
    if opts.write_scripts {
        if let (Some(k), Some(src)) = (kind, inst.source.as_ref()) {
            fs::write(d.join(format!("init.{k}.lua")), src)?;
        }
    }
    write_children(&d, &inst.children, opts, warnings)
}

// ------------------------------------------------------------- reading

pub struct Diag {
    pub is_error: bool,
    pub msg: String,
}

pub struct Loaded {
    pub roots: Vec<Instance>,
    pub diags: Vec<Diag>,
    pub ignored: usize,
}

struct Loader {
    ignore: IgnoreSet,
    diags: Vec<Diag>,
    ignored: usize,
}

pub fn load(root: &Path) -> Loaded {
    let mut l = Loader { ignore: IgnoreSet::load(root), diags: Vec::new(), ignored: 0 };
    let mut roots = Vec::new();
    for (name, p, is_dir) in l.entries(root) {
        if matches!(name.as_str(), "Project.toml" | ".stewdioignore" | ".git") {
            continue;
        }
        if l.ignore.is_ignored(&name, is_dir) {
            l.ignored += 1;
            continue;
        }
        if !is_dir {
            continue; // loose files at the project root (README, .gitignore, ...)
        }
        if let Some(i) = l.load_dir(&p, &name, true) {
            roots.push(i);
        }
    }
    Loaded { roots, diags: l.diags, ignored: l.ignored }
}

fn init_kind(name: &str) -> Option<&'static str> {
    match name {
        "init.server.lua" => Some("server"),
        "init.client.lua" => Some("client"),
        "init.module.lua" => Some("module"),
        _ => None,
    }
}

impl Loader {
    fn err(&mut self, msg: String) {
        self.diags.push(Diag { is_error: true, msg });
    }

    fn warn(&mut self, msg: String) {
        self.diags.push(Diag { is_error: false, msg });
    }

    fn entries(&mut self, dir: &Path) -> Vec<(String, PathBuf, bool)> {
        let mut v = Vec::new();
        match fs::read_dir(dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let p = e.path();
                    let is_dir = p.is_dir();
                    v.push((name, p, is_dir));
                }
            }
            Err(e) => self.err(format!("cannot read directory {}: {e}", dir.display())),
        }
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    fn read_source(&mut self, p: &Path, rel: &str) -> Option<String> {
        match fs::read(p) {
            Ok(b) => Some(String::from_utf8_lossy(&b).into_owned()),
            Err(e) => {
                self.err(format!("{rel}: cannot read script ({e})"));
                None
            }
        }
    }

    fn read_meta(&mut self, path: &Path, rel: &str) -> Option<Value> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.err(format!("{rel}: cannot read instance metadata ({e})"));
                return None;
            }
        };
        match hjson::parse(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                self.err(format!("Invalid instance metadata in {rel}: {e}"));
                None
            }
        }
    }

    fn load_dir(&mut self, path: &Path, rel: &str, top: bool) -> Option<Instance> {
        let dirname = path.file_name()?.to_string_lossy().to_string();
        let meta_path = path.join("Instance.data.hjson");
        let meta: Option<Value> = if meta_path.is_file() {
            match self.read_meta(&meta_path, rel) {
                Some(v) => Some(v),
                None => return None,
            }
        } else {
            None
        };
        if meta.is_none() {
            if top {
                self.warn(format!("{rel}: directory has no Instance.data.hjson and was skipped"));
                return None;
            }
            self.warn(format!("{rel}: no Instance.data.hjson, treating as a Folder"));
        }

        let mut init_src: Option<(String, String)> = None;
        let mut children = Vec::new();
        let entries = self.entries(path);
        let names: HashSet<String> = entries.iter().map(|e| e.0.clone()).collect();
        for (name, p, is_dir) in entries {
            let child_rel = format!("{rel}/{name}");
            if self.ignore.is_ignored(&child_rel, is_dir) {
                self.ignored += 1;
                continue;
            }
            if is_dir {
                if let Some(c) = self.load_dir(&p, &child_rel, false) {
                    children.push(c);
                }
                continue;
            }
            if name == "Instance.data.hjson" {
                continue;
            }
            if let Some(kind) = init_kind(&name) {
                if let Some(src) = self.read_source(&p, &child_rel) {
                    init_src = Some((kind.to_string(), src));
                }
                continue;
            }
            if let Some(stem) = name.strip_suffix(".data.hjson") {
                if !names.contains(&format!("{stem}.lua")) {
                    self.warn(format!("{child_rel}: metadata file has no matching .lua script"));
                }
                continue;
            }
            if let Some(stem) = name.strip_suffix(".lua") {
                if let Some(c) = self.load_script(&p, &child_rel, stem, path) {
                    children.push(c);
                }
                continue;
            }
            self.warn(format!("{child_rel}: unsupported file ignored"));
        }

        let default_class: String = match (&init_src, &meta) {
            (Some((k, _)), _) => class_for_kind(k).unwrap_or("ModuleScript").to_string(),
            (None, Some(_)) => String::new(),
            (None, None) => "Folder".to_string(),
        };
        let mut inst = self.build_instance(meta.as_ref(), &default_class, &dirname, rel)?;
        if let Some((_, src)) = init_src {
            inst.source = Some(src);
        }
        inst.children = children;
        Some(inst)
    }

    fn load_script(&mut self, p: &Path, rel: &str, stem: &str, dir: &Path) -> Option<Instance> {
        let (name_stem, kind) = if let Some(s) = stem.strip_suffix(".server") {
            (s, "server")
        } else if let Some(s) = stem.strip_suffix(".client") {
            (s, "client")
        } else if let Some(s) = stem.strip_suffix(".module") {
            (s, "module")
        } else {
            self.warn(format!("{rel}: plain .lua file treated as a ModuleScript"));
            (stem, "module")
        };
        let src = self.read_source(p, rel)?;
        let sidecar = dir.join(format!("{stem}.data.hjson"));
        let meta: Option<Value> = if sidecar.is_file() {
            match self.read_meta(&sidecar, rel) {
                Some(v) => Some(v),
                None => return None,
            }
        } else {
            None
        };
        let default_class = class_for_kind(kind).unwrap_or("ModuleScript");
        let mut inst = self.build_instance(meta.as_ref(), default_class, name_stem, rel)?;
        inst.source = Some(src);
        Some(inst)
    }

    fn build_instance(
        &mut self,
        meta: Option<&Value>,
        default_class: &str,
        default_name: &str,
        rel: &str,
    ) -> Option<Instance> {
        let mut class = default_class.to_string();
        let mut name = default_name.to_string();
        let mut id: Option<String> = None;
        let mut props: Vec<Property> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        let mut source: Option<String> = None;

        if let Some(m) = meta {
            if !matches!(m, Value::Object(_)) {
                self.err(format!("{rel}: instance metadata must be an object"));
                return None;
            }
            match m.get("$className") {
                Some(Value::Str(s)) => class = s.clone(),
                Some(_) => {
                    self.err(format!("{rel}: $className must be a string"));
                    return None;
                }
                None => {
                    if class.is_empty() {
                        self.err(format!("{rel}: invalid instance metadata, missing $className"));
                        return None;
                    }
                }
            }
            if let Some(Value::Str(s)) = m.get("$name") {
                name = s.clone();
            }
            if let Some(Value::Str(s)) = m.get("$id") {
                id = Some(s.clone());
            }
            match m.get("Properties") {
                Some(Value::Object(o)) => {
                    for (k, v) in o {
                        if k == "Name" {
                            if let Value::Str(s) = v {
                                name = s.clone();
                            }
                            continue;
                        }
                        if k == "Source" && is_script_class(&class) {
                            if let Value::Str(s) = v {
                                source = Some(s.clone());
                                continue;
                            }
                        }
                        match hjson_to_prop(k, v) {
                            Ok(p) => props.push(p),
                            Err(e) => self.err(format!("{rel}: {e}")),
                        }
                    }
                }
                Some(_) => self.err(format!("{rel}: Properties must be an object")),
                None => {}
            }
            if let Some(Value::Object(a)) = m.get("Attributes") {
                if !a.is_empty() {
                    self.warn(format!("{rel}: attributes are not serialized in v0.1 and will be dropped"));
                }
            }
            match m.get("Tags") {
                Some(Value::Array(t)) => {
                    for x in t {
                        match x {
                            Value::Str(s) => tags.push(s.clone()),
                            _ => self.err(format!("{rel}: Tags must be strings")),
                        }
                    }
                }
                Some(_) => self.err(format!("{rel}: Tags must be an array")),
                None => {}
            }
        }

        let id = id.unwrap_or_else(|| format!("auto:{rel}"));
        Some(Instance { id, class, name, properties: props, tags, source, children: Vec::new() })
    }
}
