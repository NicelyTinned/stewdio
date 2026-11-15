use std::collections::HashMap;

use crate::fsproj::Loaded;
use crate::model::*;

pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn validate(loaded: &Loaded) -> Report {
    let mut rep = Report { errors: Vec::new(), warnings: Vec::new() };
    for d in &loaded.diags {
        if d.is_error {
            rep.errors.push(d.msg.clone());
        } else {
            rep.warnings.push(d.msg.clone());
        }
    }
    let mut ids: HashMap<String, String> = HashMap::new();
    for r in &loaded.roots {
        walk(r, "", &mut ids, &mut rep);
    }
    for r in &loaded.roots {
        walk_refs(r, "", &ids, loaded.ignored, &mut rep);
    }
    rep
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn walk(inst: &Instance, parent: &str, ids: &mut HashMap<String, String>, rep: &mut Report) {
    let path = join(parent, &inst.name);
    if let Some(prev) = ids.get(&inst.id) {
        rep.errors.push(format!("Duplicate instance identifier '{}' ({} and {})", inst.id, prev, path));
    } else {
        ids.insert(inst.id.clone(), path.clone());
    }
    if !valid_ident(&inst.class) {
        rep.errors.push(format!("{path}: invalid class name '{}'", inst.class));
    }
    if is_script_class(&inst.class) && inst.source.is_none() {
        rep.warnings.push(format!("{path}: missing script file (script will be empty)"));
    }
    for p in &inst.properties {
        check_prop(&path, p, rep);
    }
    for c in &inst.children {
        walk(c, &path, ids, rep);
    }
}

fn check_prop(path: &str, p: &Property, rep: &mut Report) {
    if !valid_ident(&p.ty) {
        rep.errors.push(format!("{path}: property '{}' has an invalid type '{}'", p.name, p.ty));
        return;
    }
    if !KNOWN_TYPES.contains(&p.ty.as_str()) {
        rep.warnings.push(format!(
            "{path}: property '{}' has unrecognized type '{}' (written as-is)",
            p.name, p.ty
        ));
    }
    let scalar = matches!(
        p.ty.as_str(),
        "bool" | "string" | "int" | "int64" | "float" | "double" | "token" | "ProtectedString"
            | "BinaryString" | "Ref"
    );
    match &p.value {
        PropValue::Fields(_) if scalar => {
            rep.errors.push(format!("{path}: property '{}' of type {} expects a scalar value", p.name, p.ty));
        }
        PropValue::Text(t) => {
            let bad = match p.ty.as_str() {
                "bool" => t != "true" && t != "false",
                "int" | "int64" | "token" => t.trim().parse::<i64>().is_err(),
                "float" | "double" => t.trim().parse::<f64>().is_err(),
                _ => false,
            };
            if bad {
                rep.errors.push(format!("{path}: property '{}' has an invalid {} value '{}'", p.name, p.ty, t));
            }
        }
        _ => {}
    }
}

fn walk_refs(inst: &Instance, parent: &str, ids: &HashMap<String, String>, ignored: usize, rep: &mut Report) {
    let path = join(parent, &inst.name);
    for p in &inst.properties {
        if p.ty != "Ref" {
            continue;
        }
        if let PropValue::Text(t) = &p.value {
            if t != "null" && !ids.contains_key(t) {
                let mut msg = format!("Missing instance reference '{}' in {}.{}", t, path, p.name);
                if ignored > 0 {
                    msg.push_str(&format!(
                        " ({ignored} path(s) are excluded by .stewdioignore; make sure the target is not ignored)"
                    ));
                }
                rep.errors.push(msg);
            }
        }
    }
    for c in &inst.children {
        walk_refs(c, &path, ids, ignored, rep);
    }
}
