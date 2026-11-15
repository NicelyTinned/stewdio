//! Roblox XML place format (.rbxlx) <-> Instance tree.
use std::collections::HashMap;

use crate::model::*;
use crate::util;
use crate::xml::{self, Element, Node};

pub struct Decoded {
    pub roots: Vec<Instance>,
    pub warnings: Vec<String>,
}

struct Ctx {
    counter: usize,
    refs: HashMap<String, String>,
    warnings: Vec<String>,
}

pub fn decode(text: &str) -> Result<Decoded, String> {
    let root = xml::parse(text)?;
    if root.name != "roblox" {
        return Err("root element is not <roblox>".to_string());
    }
    let mut ctx = Ctx { counter: 0, refs: HashMap::new(), warnings: Vec::new() };
    let mut roots = Vec::new();
    for el in root.elements() {
        match el.name.as_str() {
            "Item" => {
                let inst = ctx.item(el);
                roots.push(inst);
            }
            "SharedStrings" => ctx
                .warnings
                .push("<SharedStrings> are not preserved in v0.1".to_string()),
            _ => {}
        }
    }
    for r in roots.iter_mut() {
        ctx.resolve(r);
    }
    Ok(Decoded { roots, warnings: ctx.warnings })
}

fn element_value(p: &Element) -> PropValue {
    if p.has_elements() {
        PropValue::Fields(p.elements().map(|c| (c.name.clone(), element_value(c))).collect())
    } else {
        PropValue::Text(p.text())
    }
}

impl Ctx {
    fn item(&mut self, el: &Element) -> Instance {
        self.counter += 1;
        let id = format!("instance-{:03}", self.counter);
        if let Some(r) = el.attr("referent") {
            self.refs.insert(r.to_string(), id.clone());
        }
        let class = el.attr("class").unwrap_or("Instance").to_string();
        let mut inst = Instance::new(&id, &class, &class);
        for child in el.elements() {
            match child.name.as_str() {
                "Properties" => self.properties(child, &mut inst),
                "Item" => {
                    let c = self.item(child);
                    inst.children.push(c);
                }
                _ => {}
            }
        }
        inst
    }

    fn properties(&mut self, props: &Element, inst: &mut Instance) {
        for p in props.elements() {
            let pname = match p.attr("name") {
                Some(n) => n.to_string(),
                None => {
                    self.warnings
                        .push(format!("{}: property <{}> without a name skipped", inst.name, p.name));
                    continue;
                }
            };
            let ty = p.name.clone();
            if pname == "Name" && ty == "string" {
                inst.name = p.text();
                continue;
            }
            if pname == "Source"
                && is_script_class(&inst.class)
                && (ty == "ProtectedString" || ty == "string")
            {
                inst.source = Some(p.text());
                continue;
            }
            if pname == "Tags" && ty == "BinaryString" {
                if let Some(bytes) = util::base64_decode(p.text().trim()) {
                    if let Ok(s) = String::from_utf8(bytes) {
                        inst.tags = s
                            .split('\0')
                            .filter(|t| !t.is_empty())
                            .map(|t| t.to_string())
                            .collect();
                        continue;
                    }
                }
            }
            inst.properties.push(Property { name: pname, ty, value: element_value(p) });
        }
    }

    fn resolve(&mut self, inst: &mut Instance) {
        for p in inst.properties.iter_mut() {
            if p.ty != "Ref" {
                continue;
            }
            if let PropValue::Text(t) = &mut p.value {
                let r = t.trim().to_string();
                if r.is_empty() || r == "null" {
                    *t = "null".to_string();
                } else if let Some(id) = self.refs.get(&r) {
                    *t = id.clone();
                } else {
                    self.warnings.push(format!(
                        "{}: dangling reference '{}' in property {} reset to null",
                        inst.name, r, p.name
                    ));
                    *t = "null".to_string();
                }
            }
        }
        for c in inst.children.iter_mut() {
            self.resolve(c);
        }
    }
}

// ---------------------------------------------------------------- encode

fn assign(i: &Instance, ids: &mut HashMap<String, String>, n: &mut u128) {
    *n += 1;
    ids.insert(i.id.clone(), format!("RBX{:032X}", *n));
    for c in &i.children {
        assign(c, ids, n);
    }
}

pub fn encode(roots: &[Instance]) -> Result<String, String> {
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut n: u128 = 0;
    for r in roots {
        assign(r, &mut ids, &mut n);
    }
    let mut root = Element::new("roblox")
        .with_attr("xmlns:xmime", "http://www.w3.org/2005/05/xmlmime")
        .with_attr("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance")
        .with_attr("xsi:noNamespaceSchemaLocation", "http://www.roblox.com/roblox.xsd")
        .with_attr("version", "4");
    root.children.push(Node::Element(Element::leaf("Meta", "true").with_attr("name", "ExplicitAutoJoints")));
    root.children.push(Node::Element(Element::leaf("External", "null")));
    root.children.push(Node::Element(Element::leaf("External", "nil")));
    for r in roots {
        root.children.push(Node::Element(encode_item(r, &ids)?));
    }
    Ok(root.to_document())
}

fn encode_item(i: &Instance, ids: &HashMap<String, String>) -> Result<Element, String> {
    let referent = ids.get(&i.id).cloned().unwrap_or_default();
    let mut el = Element::new("Item").with_attr("class", &i.class).with_attr("referent", &referent);
    let mut props = Element::new("Properties");
    props
        .children
        .push(Node::Element(Element::leaf("string", &i.name).with_attr("name", "Name")));
    for p in &i.properties {
        props.children.push(Node::Element(encode_prop(i, p, ids)?));
    }
    if let Some(src) = &i.source {
        let mut s = Element::new("ProtectedString").with_attr("name", "Source");
        s.children.push(Node::CData(src.clone()));
        props.children.push(Node::Element(s));
    }
    if !i.tags.is_empty() {
        let joined = i.tags.join("\0");
        props.children.push(Node::Element(
            Element::leaf("BinaryString", &util::base64_encode(joined.as_bytes())).with_attr("name", "Tags"),
        ));
    }
    el.children.push(Node::Element(props));
    for c in &i.children {
        el.children.push(Node::Element(encode_item(c, ids)?));
    }
    Ok(el)
}

fn encode_prop(inst: &Instance, p: &Property, ids: &HashMap<String, String>) -> Result<Element, String> {
    let mut el = Element::new(&p.ty).with_attr("name", &p.name);
    match &p.value {
        PropValue::Text(t) => {
            let text = if p.ty == "Ref" && t != "null" {
                match ids.get(t) {
                    Some(r) => r.clone(),
                    None => {
                        return Err(format!(
                            "Missing instance reference '{}' in {}.{}",
                            t, inst.name, p.name
                        ))
                    }
                }
            } else {
                t.clone()
            };
            if p.ty == "ProtectedString" {
                el.children.push(Node::CData(text));
            } else {
                el.children.push(Node::Text(text));
            }
        }
        PropValue::Fields(f) => {
            for (k, v) in f {
                el.children.push(Node::Element(encode_field(k, v)));
            }
        }
    }
    Ok(el)
}

fn encode_field(name: &str, v: &PropValue) -> Element {
    let mut el = Element::new(name);
    match v {
        PropValue::Text(t) => el.children.push(Node::Text(t.clone())),
        PropValue::Fields(f) => {
            for (k, x) in f {
                el.children.push(Node::Element(encode_field(k, x)));
            }
        }
    }
    el
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hjson;

    const SAMPLE: &str = "<roblox version=\"4\"><Item class=\"Workspace\" referent=\"RBX1\"><Properties><string name=\"Name\">Workspace</string></Properties><Item class=\"Part\" referent=\"RBX2\"><Properties><string name=\"Name\">P</string><bool name=\"Anchored\">true</bool><Vector3 name=\"size\"><X>1</X><Y>2</Y><Z>3</Z></Vector3><Ref name=\"PrimaryPart\">RBX2</Ref></Properties></Item><Item class=\"Script\" referent=\"RBX3\"><Properties><string name=\"Name\">S</string><ProtectedString name=\"Source\"><![CDATA[print(1)]]></ProtectedString></Properties></Item></Item></roblox>";

    #[test]
    fn decodes_and_encodes() {
        let d = decode(SAMPLE).unwrap();
        assert_eq!(d.roots.len(), 1);
        let part = &d.roots[0].children[0];
        assert_eq!(part.name, "P");
        assert_eq!(part.properties.len(), 3);
        match &part.properties[2].value {
            PropValue::Text(t) => assert_eq!(t, &part.id),
            _ => panic!("ref should be text"),
        }
        assert_eq!(d.roots[0].children[1].source.as_deref(), Some("print(1)"));
        let out = encode(&d.roots).unwrap();
        assert!(out.contains("class=\"Part\""));
        assert!(out.contains("print(1)"));
        let d2 = decode(&out).unwrap();
        assert_eq!(d2.roots[0].count(), 3);
    }

    #[test]
    fn property_hjson_round_trip() {
        let d = decode(SAMPLE).unwrap();
        for p in &d.roots[0].children[0].properties {
            let v = crate::model::prop_to_hjson(p);
            let text = hjson::to_string(&v);
            let back = crate::model::hjson_to_prop(&p.name, &hjson::parse(&text).unwrap()).unwrap();
            assert_eq!(back.ty, p.ty);
        }
    }
}
