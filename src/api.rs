//! Roblox API database (classes, properties, enums) and property autofill.
//!
//! `stewdio api update` downloads the current API dump published by the Roblox-Client-Tracker
//! repository and caches a compact index. When the index is present, compiling will:
//!   * validate class and property names (with "did you mean" suggestions),
//!   * coerce loosely written values to the exact type Roblox expects
//!     (enum names -> tokens, integers -> floats, [r,g,b] / "#rrggbb" -> Color3, ids -> Refs, ...),
//!   * optionally (`compiler.autofill = "defaults"`) add default values the dump provides.
//! Without the index only the built-in aliases (Size/Position/Color) are applied.
use clap::{Args, Subcommand};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::error::*;
use crate::model::*;
use crate::{config, hjson, http, ui, util};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Off,
    Normalize,
    Defaults,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "off" => Mode::Off,
            "defaults" => Mode::Defaults,
            _ => Mode::Normalize,
        }
    }
}

pub struct ApiProp {
    pub cat: String,
    pub ty: String,
    pub save: bool,
    pub read_only: bool,
    pub default: Option<String>,
    pub category: String,
}

pub struct ApiClass {
    pub superclass: String,
    pub props: HashMap<String, ApiProp>,
}

pub struct Api {
    pub version: String,
    pub classes: HashMap<String, ApiClass>,
    pub enums: HashMap<String, Vec<(String, i64)>>,
}

fn index_path() -> PathBuf {
    util::config_dir().join("api").join("api.idx")
}

fn clean(s: &str) -> String {
    s.replace(|c: char| c == '\t' || c == '\n' || c == '\r', " ")
}

impl Api {
    pub fn load() -> Option<Api> {
        let text = fs::read_to_string(index_path()).ok()?;
        Some(Self::parse_index(&text))
    }

    pub fn parse_index(text: &str) -> Api {
        let mut api = Api { version: String::new(), classes: HashMap::new(), enums: HashMap::new() };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("V") if f.len() >= 2 => api.version = f[1..].join(" "),
                Some("C") if f.len() >= 3 => {
                    api.classes
                        .entry(f[1].to_string())
                        .or_insert_with(|| ApiClass { superclass: String::new(), props: HashMap::new() })
                        .superclass = f[2].to_string();
                }
                Some("P") if f.len() >= 8 => {
                    let class = api
                        .classes
                        .entry(f[1].to_string())
                        .or_insert_with(|| ApiClass { superclass: String::new(), props: HashMap::new() });
                    class.props.insert(
                        f[2].to_string(),
                        ApiProp {
                            cat: f[3].to_string(),
                            ty: f[4].to_string(),
                            save: f[5] == "1",
                            read_only: f[6] == "1",
                            default: f[7].strip_prefix('=').map(|d| d.to_string()),
                            category: f.get(8).map(|c| c.to_string()).unwrap_or_default(),
                        },
                    );
                }
                Some("E") if f.len() >= 4 => {
                    if let Ok(v) = f[3].parse::<i64>() {
                        api.enums.entry(f[1].to_string()).or_default().push((f[2].to_string(), v));
                    }
                }
                _ => {}
            }
        }
        api
    }

    pub fn from_dump(v: &hjson::Value) -> Result<Api, String> {
        let classes = v
            .get("Classes")
            .and_then(|x| x.as_array())
            .ok_or("the dump has no \"Classes\" array")?;
        let mut api = Api { version: String::new(), classes: HashMap::new(), enums: HashMap::new() };
        for c in classes {
            let name = match c.get("Name").and_then(|x| x.as_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let sup = c.get("Superclass").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let mut class = ApiClass { superclass: if sup == "<<<ROOT>>>" { String::new() } else { sup }, props: HashMap::new() };
            if let Some(members) = c.get("Members").and_then(|x| x.as_array()) {
                for m in members {
                    if m.get("MemberType").and_then(|x| x.as_str()) != Some("Property") {
                        continue;
                    }
                    let pname = match m.get("Name").and_then(|x| x.as_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let (cat, ty) = match m.get("ValueType") {
                        Some(vt) => (
                            vt.get("Category").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            vt.get("Name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        ),
                        None => continue,
                    };
                    let save = m
                        .get("Serialization")
                        .and_then(|s| s.get("CanSave"))
                        .and_then(|x| x.as_bool())
                        .unwrap_or(true);
                    let read_only = m
                        .get("Tags")
                        .and_then(|t| t.as_array())
                        .map(|a| a.iter().any(|x| x.as_str() == Some("ReadOnly")))
                        .unwrap_or(false);
                    let default = match m.get("Default") {
                        Some(hjson::Value::Str(s)) => Some(clean(s)),
                        Some(hjson::Value::Number(n)) => Some(n.clone()),
                        Some(hjson::Value::Bool(b)) => Some(b.to_string()),
                        _ => None,
                    };
                    let category = m.get("Category").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    class.props.insert(pname, ApiProp { cat, ty, save, read_only, default, category });
                }
            }
            api.classes.insert(name, class);
        }
        if let Some(enums) = v.get("Enums").and_then(|x| x.as_array()) {
            for e in enums {
                let name = match e.get("Name").and_then(|x| x.as_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let mut items = Vec::new();
                if let Some(a) = e.get("Items").and_then(|x| x.as_array()) {
                    for it in a {
                        if let (Some(n), Some(hjson::Value::Number(val))) = (it.get("Name").and_then(|x| x.as_str()), it.get("Value")) {
                            if let Ok(v) = val.parse::<i64>() {
                                items.push((n.to_string(), v));
                            }
                        }
                    }
                }
                api.enums.insert(name, items);
            }
        }
        Ok(api)
    }

    pub fn to_index(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("V\t{}\n", self.version));
        let mut names: Vec<&String> = self.classes.keys().collect();
        names.sort();
        for n in names {
            let c = &self.classes[n];
            s.push_str(&format!("C\t{}\t{}\n", n, c.superclass));
            let mut pn: Vec<&String> = c.props.keys().collect();
            pn.sort();
            for p in pn {
                let a = &c.props[p];
                s.push_str(&format!(
                    "P\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    n,
                    p,
                    a.cat,
                    a.ty,
                    if a.save { 1 } else { 0 },
                    if a.read_only { 1 } else { 0 },
                    match &a.default {
                        Some(d) => format!("={}", clean(d)),
                        None => String::new(),
                    },
                    clean(&a.category)
                ));
            }
        }
        let mut en: Vec<&String> = self.enums.keys().collect();
        en.sort();
        for e in en {
            for (n, v) in &self.enums[e] {
                s.push_str(&format!("E\t{e}\t{n}\t{v}\n"));
            }
        }
        s
    }

    /// Days since the cached index was downloaded.
    pub fn age_days(&self) -> Option<u64> {
        let secs = self.version.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(util::now_secs().saturating_sub(secs) / 86400)
    }

    pub fn find_prop(&self, class: &str, name: &str) -> Option<&ApiProp> {
        let mut cur = class;
        for _ in 0..64 {
            let c = self.classes.get(cur)?;
            if let Some(p) = c.props.get(name) {
                return Some(p);
            }
            if c.superclass.is_empty() {
                return None;
            }
            cur = &c.superclass;
        }
        None
    }

    pub fn is_a(&self, class: &str, target: &str) -> bool {
        let mut cur = class;
        for _ in 0..64 {
            if cur == target {
                return true;
            }
            match self.classes.get(cur) {
                Some(c) if !c.superclass.is_empty() => cur = &c.superclass,
                _ => return false,
            }
        }
        false
    }

    pub fn chain_props(&self, class: &str) -> Vec<(&str, &ApiProp)> {
        let mut out = Vec::new();
        let mut cur = class;
        for _ in 0..64 {
            match self.classes.get(cur) {
                Some(c) => {
                    for (n, p) in &c.props {
                        out.push((n.as_str(), p));
                    }
                    if c.superclass.is_empty() {
                        break;
                    }
                    cur = &c.superclass;
                }
                None => break,
            }
        }
        out
    }

    pub fn enum_label(&self, en: &str, value: i64) -> Option<&str> {
        self.enums.get(en)?.iter().find(|(_, v)| *v == value).map(|(n, _)| n.as_str())
    }

    pub fn enum_value(&self, en: &str, item: &str) -> Option<i64> {
        self.enums
            .get(en)?
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(item))
            .map(|(_, v)| *v)
    }
}

/// A friendly nudge when the cached API data is old (or missing).
pub fn stale_hint(api: Option<&Api>) -> Option<String> {
    let max: u64 = config::Global::load()
        .get("api.max_age_days")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    match api {
        None => Some("no Roblox API data cached; run `stewdio api update` for property validation and autofill".to_string()),
        Some(a) => {
            let age = a.age_days()?;
            if age > max {
                Some(format!("Roblox API data is {age} days old and may be out of date; run `stewdio api update`"))
            } else {
                None
            }
        }
    }
}

// ------------------------------------------------------------ autofill

pub struct Fill {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub filled: usize,
}

const FALLBACK_BASEPARTS: &[&str] = &[
    "Part", "MeshPart", "WedgePart", "CornerWedgePart", "TrussPart", "SpawnLocation", "Seat",
    "VehicleSeat", "UnionOperation", "NegateOperation", "IntersectOperation",
];

const SILENT: &[&str] = &[
    "AttributesSerialize", "Tags", "SourceAssetId", "UniqueId", "HistoryId", "ScriptGuid",
    "DefinesCapabilities", "Capabilities", "SecurityCapabilities", "ModelMeshCFrame", "ModelMeshSize",
    "ModelMeshData", "PhysicsData", "ChildData", "MeshData", "LODData", "InitialSize",
];

pub fn process(api: Option<&Api>, roots: &mut [Instance], mode: Mode) -> Fill {
    let mut f = Fill { errors: Vec::new(), warnings: Vec::new(), filled: 0 };
    if mode == Mode::Off {
        return f;
    }
    for r in roots.iter_mut() {
        visit(api, r, "", mode, &mut f);
    }
    f
}

fn visit(api: Option<&Api>, inst: &mut Instance, parent: &str, mode: Mode, f: &mut Fill) {
    let path = if parent.is_empty() { inst.name.clone() } else { format!("{parent}/{}", inst.name) };
    let base_part = match api {
        Some(a) if a.classes.contains_key(&inst.class) => a.is_a(&inst.class, "BasePart"),
        _ => FALLBACK_BASEPARTS.contains(&inst.class.as_str()),
    };
    if base_part {
        normalize_aliases(inst, &path, f);
    }
    if let Some(api) = api {
        if !api.classes.contains_key(&inst.class) {
            let hint = util::suggest(&inst.class, api.classes.keys().map(|s| s.as_str()))
                .map(|s| format!(" (did you mean '{s}'?)"))
                .unwrap_or_default();
            f.warnings.push(format!("{path}: unknown class '{}'{hint}", inst.class));
        } else {
            coerce_props(api, inst, &path, f);
            if mode == Mode::Defaults {
                fill_defaults(api, inst, f);
            }
        }
    }
    for c in inst.children.iter_mut() {
        visit(api, c, &path, mode, f);
    }
}

fn field_text(v: &PropValue, k: &str) -> Option<String> {
    if let PropValue::Fields(fs) = v {
        for (n, x) in fs {
            if n.as_str() == k {
                if let PropValue::Text(t) = x {
                    return Some(t.clone());
                }
            }
        }
    }
    None
}

fn normalize_aliases(inst: &mut Instance, path: &str, f: &mut Fill) {
    // Size -> size (the name Studio serializes)
    if let Some(i) = inst.properties.iter().position(|p| p.name == "Size") {
        if inst.properties.iter().any(|p| p.name == "size") {
            f.warnings.push(format!("{path}: both 'Size' and 'size' set; 'Size' ignored"));
            inst.properties.remove(i);
        } else {
            inst.properties[i].name = "size".to_string();
        }
    }
    // Position -> CFrame translation
    if let Some(i) = inst.properties.iter().position(|p| p.name == "Position") {
        let pos = inst.properties.remove(i);
        if inst.properties.iter().any(|p| p.name == "CFrame") {
            f.warnings.push(format!("{path}: 'Position' ignored because 'CFrame' is set"));
        } else {
            let g = |k: &str| field_text(&pos.value, k).unwrap_or_else(|| "0".to_string());
            let mut fields = vec![("X".to_string(), PropValue::Text(g("X"))), ("Y".to_string(), PropValue::Text(g("Y"))), ("Z".to_string(), PropValue::Text(g("Z")))];
            for (k, v) in [("R00", "1"), ("R01", "0"), ("R02", "0"), ("R10", "0"), ("R11", "1"), ("R12", "0"), ("R20", "0"), ("R21", "0"), ("R22", "1")] {
                fields.push((k.to_string(), PropValue::Text(v.to_string())));
            }
            inst.properties.push(Property { name: "CFrame".to_string(), ty: "CoordinateFrame".to_string(), value: PropValue::Fields(fields) });
        }
    }
    // Color -> Color3uint8 (packed)
    if let Some(i) = inst.properties.iter().position(|p| p.name == "Color") {
        let col = inst.properties.remove(i);
        if inst.properties.iter().any(|p| p.name == "Color3uint8") {
            f.warnings.push(format!("{path}: 'Color' ignored because 'Color3uint8' is set"));
        } else if let Some(packed) = color_to_packed(&col.value) {
            inst.properties.push(Property { name: "Color3uint8".to_string(), ty: "Color3uint8".to_string(), value: PropValue::Text(packed.to_string()) });
        } else {
            f.errors.push(format!("{path}: 'Color' must be a Color3 ({{R,G,B}}, [r,g,b] or \"#rrggbb\")"));
        }
    }
}

fn scale_channels(vals: [f32; 3]) -> [f32; 3] {
    if vals.iter().any(|v| *v > 1.0) {
        [vals[0] / 255.0, vals[1] / 255.0, vals[2] / 255.0]
    } else {
        vals
    }
}

fn channels(v: &PropValue, keys: [&str; 3]) -> Option<[f32; 3]> {
    let mut out = [0f32; 3];
    for (i, k) in keys.iter().enumerate() {
        out[i] = field_text(v, k)?.trim().parse::<f32>().ok()?;
    }
    Some(scale_channels(out))
}

fn hex_color(t: &str) -> Option<[f32; 3]> {
    let t = t.trim();
    if t.len() != 7 || !t.starts_with('#') {
        return None;
    }
    let r = u8::from_str_radix(&t[1..3], 16).ok()?;
    let g = u8::from_str_radix(&t[3..5], 16).ok()?;
    let b = u8::from_str_radix(&t[5..7], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

fn color_to_packed(v: &PropValue) -> Option<u32> {
    let c = match v {
        PropValue::Text(t) => hex_color(t)?,
        f => channels(f, ["R", "G", "B"]).or_else(|| channels(f, ["X", "Y", "Z"]))?,
    };
    let q = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u32;
    Some(0xFF00_0000 | (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2]))
}

fn color3_value(c: [f32; 3]) -> PropValue {
    PropValue::Fields(vec![
        ("R".to_string(), PropValue::Text(format!("{}", c[0]))),
        ("G".to_string(), PropValue::Text(format!("{}", c[1]))),
        ("B".to_string(), PropValue::Text(format!("{}", c[2]))),
    ])
}

pub fn dump_name(n: &str) -> &str {
    match n {
        "size" => "Size",
        "Color3uint8" => "Color",
        other => other,
    }
}

fn coerce_props(api: &Api, inst: &mut Instance, path: &str, f: &mut Fill) {
    let class = inst.class.clone();
    for p in inst.properties.iter_mut() {
        let ap = match api.find_prop(&class, dump_name(&p.name)) {
            Some(a) => a,
            None => {
                if !SILENT.contains(&p.name.as_str()) {
                    let names: Vec<&str> = api.chain_props(&class).into_iter().map(|(n, _)| n).collect();
                    let hint = util::suggest(&p.name, names.into_iter())
                        .map(|s| format!(" (did you mean '{s}'?)"))
                        .unwrap_or_default();
                    f.warnings.push(format!("{path}: unknown property '{}' on {}{hint}", p.name, class));
                }
                continue;
            }
        };
        let text = match &p.value {
            PropValue::Text(t) => Some(t.trim().to_string()),
            _ => None,
        };
        match ap.cat.as_str() {
            "Enum" => {
                if matches!(p.ty.as_str(), "string" | "int" | "token") {
                    if let Some(t) = text {
                        if t.parse::<i64>().is_ok() {
                            p.ty = "token".to_string();
                            p.value = PropValue::Text(t);
                        } else {
                            match api.enum_value(&ap.ty, &t) {
                                Some(v) => {
                                    p.ty = "token".to_string();
                                    p.value = PropValue::Text(v.to_string());
                                }
                                None => {
                                    let items = api.enums.get(&ap.ty).map(|v| v.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()).unwrap_or_default();
                                    let hint = util::suggest(&t, items.into_iter()).map(|s| format!(" (did you mean '{s}'?)")).unwrap_or_default();
                                    f.errors.push(format!("{path}: invalid {} value '{t}' for {class}.{}{hint}", ap.ty, p.name));
                                }
                            }
                        }
                    }
                }
            }
            "Primitive" => {
                if text.is_some() && matches!(p.ty.as_str(), "int" | "float") && matches!(ap.ty.as_str(), "float" | "double" | "int" | "int64") {
                    p.ty = ap.ty.clone();
                }
            }
            "Class" => {
                if p.ty == "string" {
                    p.ty = "Ref".to_string();
                }
            }
            "DataType" => match ap.ty.as_str() {
                "Color3" => {
                    let converted = match &p.value {
                        PropValue::Text(t) if p.ty == "string" => hex_color(t).map(color3_value),
                        PropValue::Fields(_) if p.ty == "Vector3" => channels(&p.value, ["X", "Y", "Z"]).map(color3_value),
                        PropValue::Fields(_) if p.ty == "Color3" => channels(&p.value, ["R", "G", "B"]).map(color3_value),
                        _ => None,
                    };
                    if let Some(v) = converted {
                        p.ty = "Color3".to_string();
                        p.value = v;
                    }
                }
                "Content" => {
                    if p.ty == "string" {
                        if let Some(t) = text {
                            p.ty = "Content".to_string();
                            p.value = if t.is_empty() {
                                PropValue::Fields(vec![("null".to_string(), PropValue::Text(String::new()))])
                            } else {
                                PropValue::Fields(vec![("url".to_string(), PropValue::Text(t))])
                            };
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

// Fallback defaults for very common properties, used when the dump has no "Default" value.
const BUILTIN_DEFAULTS: &[(&str, &str, &str)] = &[
    ("BasePart", "Anchored", "false"),
    ("BasePart", "CanCollide", "true"),
    ("BasePart", "CanTouch", "true"),
    ("BasePart", "CanQuery", "true"),
    ("BasePart", "CastShadow", "true"),
    ("BasePart", "Massless", "false"),
    ("BasePart", "Locked", "false"),
    ("BasePart", "Transparency", "0"),
    ("BasePart", "Reflectance", "0"),
    ("BasePart", "Material", "Plastic"),
    ("LuaSourceContainer", "Disabled", "false"),
    ("Script", "Disabled", "false"),
    ("Instance", "Archivable", "true"),
];

pub fn builtin_default(api: &Api, class: &str, name: &str) -> Option<&'static str> {
    BUILTIN_DEFAULTS
        .iter()
        .find(|(base, n, _)| *n == name && api.is_a(class, base))
        .map(|(_, _, d)| *d)
}

fn nums(d: &str, n: usize) -> Option<Vec<String>> {
    let v: Vec<String> = d
        .split(|c: char| c == ',' || c == '(' || c == ')' || c == '{' || c == '}' || c.is_whitespace())
        .filter(|t| !t.is_empty() && t.parse::<f64>().is_ok())
        .map(|t| t.to_string())
        .collect();
    if v.len() >= n {
        Some(v.into_iter().take(n).collect())
    } else {
        None
    }
}

fn fields(items: &[(&str, String)]) -> PropValue {
    PropValue::Fields(items.iter().map(|(k, v)| (k.to_string(), PropValue::Text(v.clone()))).collect())
}

fn parse_default(api: &Api, ap: &ApiProp, d: &str) -> Option<(String, PropValue)> {
    let t = |s: &str| PropValue::Text(s.to_string());
    match (ap.cat.as_str(), ap.ty.as_str()) {
        ("Primitive", "bool") => {
            if d == "true" || d == "false" {
                Some(("bool".to_string(), t(d)))
            } else {
                None
            }
        }
        ("Primitive", ty @ ("float" | "double")) => d.parse::<f64>().ok().map(|_| (ty.to_string(), t(d))),
        ("Primitive", ty @ ("int" | "int64")) => d.parse::<i64>().ok().map(|_| (ty.to_string(), t(d))),
        ("Primitive", "string") => Some(("string".to_string(), t(d))),
        ("Enum", en) => {
            if d.parse::<i64>().is_ok() {
                Some(("token".to_string(), t(d)))
            } else {
                api.enum_value(en, d).map(|v| ("token".to_string(), t(&v.to_string())))
            }
        }
        ("DataType", "Vector3") => nums(d, 3).map(|n| ("Vector3".to_string(), fields(&[("X", n[0].clone()), ("Y", n[1].clone()), ("Z", n[2].clone())]))),
        ("DataType", "Vector2") => nums(d, 2).map(|n| ("Vector2".to_string(), fields(&[("X", n[0].clone()), ("Y", n[1].clone())]))),
        ("DataType", "Color3") => nums(d, 3).map(|n| {
            let mut c: Vec<f32> = n.iter().map(|x| x.parse::<f32>().unwrap_or(0.0)).collect();
            if c.iter().any(|v| *v > 1.0) {
                for v in c.iter_mut() {
                    *v /= 255.0;
                }
            }
            ("Color3".to_string(), fields(&[("R", c[0].to_string()), ("G", c[1].to_string()), ("B", c[2].to_string())]))
        }),
        ("DataType", "UDim2") => nums(d, 4).map(|n| {
            ("UDim2".to_string(), fields(&[("XS", n[0].clone()), ("XO", n[1].clone()), ("YS", n[2].clone()), ("YO", n[3].clone())]))
        }),
        ("DataType", "UDim") => nums(d, 2).map(|n| ("UDim".to_string(), fields(&[("S", n[0].clone()), ("O", n[1].clone())]))),
        ("DataType", "BrickColor") => d.parse::<u32>().ok().map(|_| ("BrickColor".to_string(), t(d))),
        ("DataType", "Content") => Some((
            "Content".to_string(),
            if d.is_empty() { fields(&[("null", String::new())]) } else { fields(&[("url", d.to_string())]) },
        )),
        _ => None,
    }
}

/// The default value of a property as the dump (or the built-in table) describes it.
pub fn default_value(api: &Api, class: &str, name: &str, ap: &ApiProp) -> Option<(String, PropValue)> {
    let d = match ap.default.as_deref() {
        Some(d) => d.trim().to_string(),
        None => builtin_default(api, class, name)?.to_string(),
    };
    parse_default(api, ap, &d)
}

/// A neutral starting value (0 / false / empty) for creating a property whose default is unknown.
pub fn zero_value(api: &Api, ap: &ApiProp) -> Option<(String, PropValue)> {
    let z = |s: &str| s.to_string();
    match (ap.cat.as_str(), ap.ty.as_str()) {
        ("Primitive", "bool") => Some((z("bool"), PropValue::Text(z("false")))),
        ("Primitive", ty @ ("float" | "double" | "int" | "int64")) => Some((z(ty), PropValue::Text(z("0")))),
        ("Primitive", "string") => Some((z("string"), PropValue::Text(String::new()))),
        ("Enum", en) => {
            let v = api.enums.get(en).and_then(|i| i.first()).map(|(_, v)| *v).unwrap_or(0);
            Some((z("token"), PropValue::Text(v.to_string())))
        }
        ("DataType", "Vector3") => Some((z("Vector3"), fields(&[("X", z("0")), ("Y", z("0")), ("Z", z("0"))]))),
        ("DataType", "Vector2") => Some((z("Vector2"), fields(&[("X", z("0")), ("Y", z("0"))]))),
        ("DataType", "Color3") => Some((z("Color3"), fields(&[("R", z("0")), ("G", z("0")), ("B", z("0"))]))),
        ("DataType", "UDim2") => Some((z("UDim2"), fields(&[("XS", z("0")), ("XO", z("0")), ("YS", z("0")), ("YO", z("0"))]))),
        ("DataType", "UDim") => Some((z("UDim"), fields(&[("S", z("0")), ("O", z("0"))]))),
        ("DataType", "BrickColor") => Some((z("BrickColor"), PropValue::Text(z("194")))),
        ("DataType", "Content") => Some((z("Content"), fields(&[("null", String::new())]))),
        _ => None,
    }
}

fn fill_defaults(api: &Api, inst: &mut Instance, f: &mut Fill) {
    let mut add: Vec<Property> = Vec::new();
    for (name, ap) in api.chain_props(&inst.class) {
        // "Color" is stored as a packed Color3uint8 and is handled by normalization instead
        if !ap.save || ap.read_only || matches!(name, "Name" | "Source" | "Tags" | "Color") {
            continue;
        }
        let ser = if name == "Size" { "size" } else { name };
        if inst.properties.iter().any(|p| p.name == ser || p.name == name) || add.iter().any(|p| p.name == ser) {
            continue;
        }
        if let Some((ty, value)) = default_value(api, &inst.class, name, ap) {
            add.push(Property { name: ser.to_string(), ty, value });
        }
    }
    add.sort_by(|a, b| a.name.cmp(&b.name));
    f.filled += add.len();
    inst.properties.extend(add);
}

// ------------------------------------------------------------- command

#[derive(Args)]
pub struct ApiArgs {
    #[command(subcommand)]
    pub cmd: ApiCmd,
}

#[derive(Subcommand)]
pub enum ApiCmd {
    /// Download the current Roblox API dump and cache it
    Update {
        /// Use a different dump URL
        #[arg(long)]
        url: Option<String>,
    },
    /// Show the cached API database
    Status,
}

pub fn run(a: ApiArgs) -> CliResult<()> {
    match a.cmd {
        ApiCmd::Update { url } => {
            let g = config::Global::load();
            let url = url
                .or_else(|| g.get("api.url"))
                .unwrap_or_else(|| config::DEFAULT_API_URL.to_string());
            ui::step(&format!("Downloading {url}"));
            let resp = http::request("GET", &url, &[], None, None).map_err(|e| CliError::new(GENERAL, e))?;
            if resp.status != 200 {
                return Err(CliError::new(GENERAL, format!("Download failed (HTTP {})", resp.status)));
            }
            ui::step("Parsing API dump...");
            let v = hjson::parse(&String::from_utf8_lossy(&resp.body))
                .map_err(|e| CliError::new(PARSE, format!("Invalid API dump: {e}")))?;
            let mut api = Api::from_dump(&v).map_err(|e| CliError::new(PARSE, e))?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            api.version = format!("{now} {url}");
            let _ = now;
            let p = index_path();
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&p, api.to_index())?;
            let props: usize = api.classes.values().map(|c| c.props.len()).sum();
            let defaults: usize = api.classes.values().flat_map(|c| c.props.values()).filter(|p| p.default.is_some()).count();
            ui::success("API database updated");
            println!("Classes: {}", api.classes.len());
            println!("Properties: {props} ({defaults} with default values)");
            println!("Enums: {}", api.enums.len());
            if defaults == 0 {
                ui::hint("this dump has no default values, so `compiler.autofill = \"defaults\"` will add nothing");
            }
        }
        ApiCmd::Status => match Api::load() {
            Some(api) => {
                let props: usize = api.classes.values().map(|c| c.props.len()).sum();
                println!("Cached: {}", index_path().display());
                println!("Source: {}", api.version);
                if let Some(d) = api.age_days() {
                    println!("Age: {d} day(s)");
                }
                if let Some(h) = stale_hint(Some(&api)) {
                    ui::hint(&h);
                }
                println!("Classes: {}  Properties: {}  Enums: {}", api.classes.len(), props, api.enums.len());
            }
            None => {
                println!("No API database cached");
                ui::hint("run `stewdio api update`");
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> Api {
        Api::parse_index("C\tBasePart\t\nC\tPart\tBasePart\nP\tBasePart\tMaterial\tEnum\tMaterial\t1\t0\t\nP\tBasePart\tTransparency\tPrimitive\tfloat\t1\t0\t\nP\tBasePart\tColor\tDataType\tColor3\t1\t0\t\nE\tMaterial\tPlastic\t256\nE\tMaterial\tWood\t512\n")
    }

    fn prop(name: &str, ty: &str, v: &str) -> Property {
        Property { name: name.to_string(), ty: ty.to_string(), value: PropValue::Text(v.to_string()) }
    }

    #[test]
    fn coerces_enums_and_numbers() {
        let a = api();
        let mut inst = Instance::new("i", "Part", "P");
        inst.properties.push(prop("Material", "string", "Wood"));
        inst.properties.push(prop("Transparency", "int", "1"));
        let mut roots = vec![inst];
        let f = process(Some(&a), &mut roots, Mode::Normalize);
        assert!(f.errors.is_empty());
        assert_eq!(roots[0].properties[0].ty, "token");
        match &roots[0].properties[0].value {
            PropValue::Text(t) => assert_eq!(t, "512"),
            _ => panic!(),
        }
        assert_eq!(roots[0].properties[1].ty, "float");
    }

    #[test]
    fn reports_bad_enum_with_suggestion() {
        let a = api();
        let mut inst = Instance::new("i", "Part", "P");
        inst.properties.push(prop("Material", "string", "Woood"));
        let mut roots = vec![inst];
        let f = process(Some(&a), &mut roots, Mode::Normalize);
        assert_eq!(f.errors.len(), 1);
        assert!(f.errors[0].contains("Wood"));
    }

    #[test]
    fn aliases_work_without_database() {
        let mut inst = Instance::new("i", "Part", "P");
        inst.properties.push(Property {
            name: "Position".to_string(),
            ty: "Vector3".to_string(),
            value: PropValue::Fields(vec![
                ("X".to_string(), PropValue::Text("1".to_string())),
                ("Y".to_string(), PropValue::Text("2".to_string())),
                ("Z".to_string(), PropValue::Text("3".to_string())),
            ]),
        });
        let mut roots = vec![inst];
        process(None, &mut roots, Mode::Normalize);
        assert_eq!(roots[0].properties[0].name, "CFrame");
        assert_eq!(roots[0].properties[0].ty, "CoordinateFrame");
    }
}
