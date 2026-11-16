use crate::hjson::Value;

pub const SERVICES: &[&str] = &[
    "Workspace",
    "Players",
    "Lighting",
    "ReplicatedFirst",
    "ReplicatedStorage",
    "ServerScriptService",
    "ServerStorage",
    "StarterGui",
    "StarterPack",
    "StarterPlayer",
    "SoundService",
];

pub const KNOWN_TYPES: &[&str] = &[
    "bool", "string", "int", "int64", "float", "double", "token", "Vector3", "Vector2",
    "Vector3int16", "Vector2int16", "CoordinateFrame", "OptionalCoordinateFrame", "Color3",
    "Color3uint8", "Content", "ProtectedString", "BinaryString", "Ref", "UDim", "UDim2", "Rect2D",
    "Rect", "NumberRange", "NumberSequence", "ColorSequence", "PhysicalProperties", "Faces", "Axes",
    "BrickColor", "Font", "Ray", "Region3", "Region3int16", "SharedString", "UniqueId",
    "SecurityCapabilities",
];

pub fn is_script_class(c: &str) -> bool {
    matches!(c, "Script" | "LocalScript" | "ModuleScript")
}

pub fn script_kind(class: &str) -> Option<&'static str> {
    match class {
        "Script" => Some("server"),
        "LocalScript" => Some("client"),
        "ModuleScript" => Some("module"),
        _ => None,
    }
}

pub fn class_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "server" => Some("Script"),
        "client" => Some("LocalScript"),
        "module" => Some("ModuleScript"),
        _ => None,
    }
}

/// Property value as stored in XML: scalar text or nested named fields.
#[derive(Debug, Clone)]
pub enum PropValue {
    Text(String),
    Fields(Vec<(String, PropValue)>),
}

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub ty: String,
    pub value: PropValue,
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: String,
    pub class: String,
    pub name: String,
    pub properties: Vec<Property>,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub children: Vec<Instance>,
}

impl Instance {
    pub fn new(id: &str, class: &str, name: &str) -> Self {
        Instance {
            id: id.to_string(),
            class: class.to_string(),
            name: name.to_string(),
            properties: Vec::new(),
            tags: Vec::new(),
            source: None,
            children: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(|c| c.count()).sum::<usize>()
    }

    pub fn script_count(&self) -> usize {
        (if is_script_class(&self.class) { 1 } else { 0 })
            + self.children.iter().map(|c| c.script_count()).sum::<usize>()
    }
}

// ------------------------------------------------------ HJSON conversion

fn text_value(t: &str) -> Value {
    if crate::util::is_json_number(t) {
        Value::Number(t.to_string())
    } else {
        Value::Str(t.to_string())
    }
}

fn field_to_hjson(v: &PropValue) -> Value {
    match v {
        PropValue::Text(t) => text_value(t),
        PropValue::Fields(f) => {
            Value::Object(f.iter().map(|(k, x)| (k.clone(), field_to_hjson(x))).collect())
        }
    }
}

pub fn prop_to_hjson(p: &Property) -> Value {
    match &p.value {
        PropValue::Text(t) => match p.ty.as_str() {
            "bool" if t == "true" || t == "false" => Value::Bool(t == "true"),
            "string" => Value::Str(t.clone()),
            "Ref" if t == "null" => Value::Null,
            _ => Value::Object(vec![
                ("$type".to_string(), Value::Str(p.ty.clone())),
                ("$value".to_string(), text_value(t)),
            ]),
        },
        PropValue::Fields(f) => {
            let mut o = vec![("$type".to_string(), Value::Str(p.ty.clone()))];
            for (k, x) in f {
                o.push((k.clone(), field_to_hjson(x)));
            }
            Value::Object(o)
        }
    }
}

fn scalar_text(v: &Value, ty: &str) -> Result<String, String> {
    match v {
        Value::Number(n) => Ok(n.clone()),
        Value::Str(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null if ty == "Ref" => Ok("null".to_string()),
        _ => Err("$value must be a number, string or boolean".to_string()),
    }
}

fn field_from_hjson(v: &Value) -> Result<PropValue, String> {
    match v {
        Value::Number(n) => Ok(PropValue::Text(n.clone())),
        Value::Str(s) => Ok(PropValue::Text(s.clone())),
        Value::Bool(b) => Ok(PropValue::Text(b.to_string())),
        Value::Object(o) => {
            let mut f = Vec::new();
            for (k, x) in o {
                f.push((k.clone(), field_from_hjson(x).map_err(|e| format!("{k}: {e}"))?));
            }
            Ok(PropValue::Fields(f))
        }
        Value::Null => Err("null is not a supported value here".to_string()),
        Value::Array(_) => Err("arrays are not supported here".to_string()),
    }
}

fn infer_type(o: &[(String, Value)]) -> Option<String> {
    let mut keys: Vec<&str> = o.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    match keys.as_slice() {
        ["X", "Y", "Z"] => Some("Vector3".to_string()),
        ["X", "Y"] => Some("Vector2".to_string()),
        ["B", "G", "R"] => Some("Color3".to_string()),
        _ => None,
    }
}

pub fn hjson_to_prop(name: &str, v: &Value) -> Result<Property, String> {
    let mk = |ty: &str, t: String| Property {
        name: name.to_string(),
        ty: ty.to_string(),
        value: PropValue::Text(t),
    };
    match v {
        Value::Bool(b) => Ok(mk("bool", b.to_string())),
        Value::Str(s) => Ok(mk("string", s.clone())),
        Value::Number(n) => {
            let ty = if n.contains(|c: char| c == '.' || c == 'e' || c == 'E') { "float" } else { "int" };
            Ok(mk(ty, n.clone()))
        }
        Value::Null => Ok(mk("Ref", "null".to_string())),
        Value::Array(_) => Err(format!("property '{name}': arrays are not supported")),
        Value::Object(o) => {
            let explicit = match v.get("$type") {
                Some(Value::Str(t)) => Some(t.clone()),
                Some(_) => return Err(format!("property '{name}': $type must be a string")),
                None => None,
            };
            let ty = match explicit {
                Some(t) => t,
                None => match infer_type(o) {
                    Some(t) => t,
                    None => {
                        return Err(format!(
                            "property '{name}': missing $type and the type could not be inferred"
                        ))
                    }
                },
            };
            if let Some(val) = v.get("$value") {
                let text = scalar_text(val, &ty).map_err(|e| format!("property '{name}': {e}"))?;
                return Ok(mk(&ty, text));
            }
            let mut fields = Vec::new();
            for (k, fv) in o {
                if k == "$type" {
                    continue;
                }
                let f = field_from_hjson(fv).map_err(|e| format!("property '{name}.{k}': {e}"))?;
                fields.push((k.clone(), f));
            }
            Ok(Property { name: name.to_string(), ty, value: PropValue::Fields(fields) })
        }
    }
}

/// Add a decoded property to an instance, routing Name / Source / Tags to their dedicated fields.
pub fn absorb(inst: &mut Instance, p: Property) {
    if p.name == "Name" && p.ty == "string" {
        if let PropValue::Text(t) = p.value {
            inst.name = t;
        }
        return;
    }
    if p.name == "Source"
        && is_script_class(&inst.class)
        && (p.ty == "ProtectedString" || p.ty == "string")
    {
        if let PropValue::Text(t) = p.value {
            inst.source = Some(t);
        }
        return;
    }
    if p.name == "Tags" && p.ty == "BinaryString" {
        if let PropValue::Text(t) = &p.value {
            if let Some(bytes) = crate::util::base64_decode(t.trim()) {
                if let Ok(s) = String::from_utf8(bytes) {
                    inst.tags = s
                        .split('\0')
                        .filter(|x| !x.is_empty())
                        .map(|x| x.to_string())
                        .collect();
                    return;
                }
            }
        }
    }
    inst.properties.push(p);
}
