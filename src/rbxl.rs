//! Roblox binary place format (.rbxl) <-> Instance tree.
//!
//! Chunks: META, SSTR, INST, PROP, PRNT, END. Payloads are LZ4 block compressed.
//! Property values are converted to/from the XML-flavoured model used everywhere else
//! (type name + scalar text or named fields), which keeps the project format independent
//! of the on-disk place format.
use std::collections::HashMap;

use crate::lz4;
use crate::model::*;
use crate::rbxlx::Decoded;
use crate::util;

const MAGIC: &[u8] = b"<roblox!\x89\xff\r\n\x1a\n";

/// String-typed properties whose XML type is BinaryString.
const BINARY_NAMES: &[&str] = &[
    "Tags",
    "AttributesSerialize",
    "MaterialColors",
    "SmoothGrid",
    "PhysicsGrid",
    "ClusterGridV3",
];

/// String-typed properties whose XML type is Content.
const CONTENT_NAMES: &[&str] = &[
    "MeshId", "TextureID", "TextureId", "Texture", "SoundId", "Image", "SkyboxBk", "SkyboxDn",
    "SkyboxFt", "SkyboxLf", "SkyboxRt", "SkyboxUp", "ColorMap", "MetalnessMap", "NormalMap",
    "RoughnessMap", "SunTextureId", "MoonTextureId", "ShirtTemplate", "PantsTemplate", "Graphic",
    "AnimationId", "LinkedSource", "PlaceholderImage", "OverlayTextureId", "BaseTextureId",
    "ImageContent", "ThumbnailImage", "HoverImage", "PressedImage", "SelectionImage",
];

const R_KEYS: [&str; 9] = ["R00", "R01", "R02", "R10", "R11", "R12", "R20", "R21", "R22"];

/// Axis-aligned CFrame rotation ids (row-major R00..R22).
const ROTATIONS: [(u8, [i8; 9]); 24] = [
    (0x02, [1, 0, 0, 0, 1, 0, 0, 0, 1]),
    (0x03, [1, 0, 0, 0, 0, -1, 0, 1, 0]),
    (0x05, [1, 0, 0, 0, -1, 0, 0, 0, -1]),
    (0x06, [1, 0, 0, 0, 0, 1, 0, -1, 0]),
    (0x07, [0, 1, 0, 1, 0, 0, 0, 0, -1]),
    (0x09, [0, 0, 1, 1, 0, 0, 0, 1, 0]),
    (0x0a, [0, -1, 0, 1, 0, 0, 0, 0, 1]),
    (0x0c, [0, 0, -1, 1, 0, 0, 0, -1, 0]),
    (0x0d, [0, 1, 0, 0, 0, 1, 1, 0, 0]),
    (0x0e, [0, 0, -1, 0, 1, 0, 1, 0, 0]),
    (0x10, [0, -1, 0, 0, 0, -1, 1, 0, 0]),
    (0x11, [0, 0, 1, 0, -1, 0, 1, 0, 0]),
    (0x14, [-1, 0, 0, 0, 1, 0, 0, 0, -1]),
    (0x15, [-1, 0, 0, 0, 0, 1, 0, 1, 0]),
    (0x17, [-1, 0, 0, 0, -1, 0, 0, 0, 1]),
    (0x18, [-1, 0, 0, 0, 0, -1, 0, -1, 0]),
    (0x19, [0, 1, 0, -1, 0, 0, 0, 0, 1]),
    (0x1b, [0, 0, -1, -1, 0, 0, 0, 1, 0]),
    (0x1c, [0, -1, 0, -1, 0, 0, 0, 0, -1]),
    (0x1e, [0, 0, 1, -1, 0, 0, 0, -1, 0]),
    (0x1f, [0, 1, 0, 0, 0, -1, -1, 0, 0]),
    (0x20, [0, 0, 1, 0, 1, 0, -1, 0, 0]),
    (0x22, [0, -1, 0, 0, 0, 1, -1, 0, 0]),
    (0x23, [0, 0, -1, 0, -1, 0, -1, 0, 0]),
];

// =============================================================== reading

struct Cur<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(d: &'a [u8]) -> Self {
        Cur { d, p: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let len = self.d.len();
        let end = match self.p.checked_add(n) {
            Some(e) if e <= len => e,
            _ => return Err("unexpected end of data".to_string()),
        };
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> Result<i16, String> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f64(&mut self) -> Result<f64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(f64::from_le_bytes(a))
    }

    fn str_bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn interleaved(&mut self, n: usize, w: usize) -> Result<Vec<u64>, String> {
        let total = match n.checked_mul(w) {
            Some(t) => t,
            None => return Err("count overflow".to_string()),
        };
        let b = self.take(total)?;
        let mut out = vec![0u64; n];
        for lane in 0..w {
            for i in 0..n {
                out[i] = (out[i] << 8) | (b[lane * n + i] as u64);
            }
        }
        Ok(out)
    }

    fn i32s(&mut self, n: usize) -> Result<Vec<i32>, String> {
        Ok(self
            .interleaved(n, 4)?
            .into_iter()
            .map(|v| {
                let v = v as u32;
                ((v >> 1) as i32) ^ (-((v & 1) as i32))
            })
            .collect())
    }

    fn u32s(&mut self, n: usize) -> Result<Vec<u32>, String> {
        Ok(self.interleaved(n, 4)?.into_iter().map(|v| v as u32).collect())
    }

    fn f32s(&mut self, n: usize) -> Result<Vec<f32>, String> {
        Ok(self
            .interleaved(n, 4)?
            .into_iter()
            .map(|v| {
                let v = v as u32;
                f32::from_bits((v >> 1) | (v << 31))
            })
            .collect())
    }

    fn i64s(&mut self, n: usize) -> Result<Vec<i64>, String> {
        Ok(self
            .interleaved(n, 8)?
            .into_iter()
            .map(|v| ((v >> 1) as i64) ^ (-((v & 1) as i64)))
            .collect())
    }

    fn referents(&mut self, n: usize) -> Result<Vec<i32>, String> {
        let d = self.i32s(n)?;
        let mut prev = 0i32;
        Ok(d.into_iter()
            .map(|x| {
                prev = prev.wrapping_add(x);
                prev
            })
            .collect())
    }
}

fn ft(f: f32) -> String {
    if f.is_nan() {
        "NAN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "INF".to_string() } else { "-INF".to_string() }
    } else {
        format!("{}", f)
    }
}

fn dt(f: f64) -> String {
    if f.is_nan() {
        "NAN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "INF".to_string() } else { "-INF".to_string() }
    } else {
        format!("{}", f)
    }
}

fn tx(s: impl Into<String>) -> PropValue {
    PropValue::Text(s.into())
}

fn fl(items: Vec<(&str, PropValue)>) -> PropValue {
    PropValue::Fields(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn same(ty: &str, vals: Vec<PropValue>) -> Option<Vec<(String, PropValue)>> {
    Some(vals.into_iter().map(|v| (ty.to_string(), v)).collect())
}

fn string_prop(pname: &str, b: &[u8]) -> (String, PropValue) {
    if BINARY_NAMES.contains(&pname) {
        return ("BinaryString".to_string(), tx(util::base64_encode(b)));
    }
    match std::str::from_utf8(b) {
        Ok(s) => {
            if pname == "Source" {
                ("ProtectedString".to_string(), tx(s))
            } else if CONTENT_NAMES.contains(&pname) {
                let v = if s.is_empty() { fl(vec![("null", tx(""))]) } else { fl(vec![("url", tx(s))]) };
                ("Content".to_string(), v)
            } else {
                ("string".to_string(), tx(s))
            }
        }
        Err(_) => ("BinaryString".to_string(), tx(util::base64_encode(b))),
    }
}

/// Returns Ok(None) for property types this version cannot interpret.
fn decode_values(
    c: &mut Cur,
    tid: u8,
    n: usize,
    pname: &str,
    sstr: &[Vec<u8>],
) -> Result<Option<Vec<(String, PropValue)>>, String> {
    Ok(match tid {
        0x01 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let b = c.str_bytes()?;
                out.push(string_prop(pname, b));
            }
            Some(out)
        }
        0x02 => {
            let b = c.take(n)?;
            same("bool", b.iter().map(|x| tx(if *x != 0 { "true" } else { "false" })).collect())
        }
        0x03 => {
            let v = c.i32s(n)?;
            same("int", v.iter().map(|x| tx(x.to_string())).collect())
        }
        0x04 => {
            let v = c.f32s(n)?;
            same("float", v.iter().map(|x| tx(ft(*x))).collect())
        }
        0x05 => {
            let mut out = Vec::new();
            for _ in 0..n {
                out.push(tx(dt(c.f64()?)));
            }
            same("double", out)
        }
        0x06 => {
            let s = c.f32s(n)?;
            let o = c.i32s(n)?;
            same(
                "UDim",
                (0..n).map(|i| fl(vec![("S", tx(ft(s[i]))), ("O", tx(o[i].to_string()))])).collect(),
            )
        }
        0x07 => {
            let xs = c.f32s(n)?;
            let ys = c.f32s(n)?;
            let xo = c.i32s(n)?;
            let yo = c.i32s(n)?;
            same(
                "UDim2",
                (0..n)
                    .map(|i| {
                        fl(vec![
                            ("XS", tx(ft(xs[i]))),
                            ("XO", tx(xo[i].to_string())),
                            ("YS", tx(ft(ys[i]))),
                            ("YO", tx(yo[i].to_string())),
                        ])
                    })
                    .collect(),
            )
        }
        0x08 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let mut f = [0f32; 6];
                for k in 0..6 {
                    f[k] = c.f32()?;
                }
                out.push(fl(vec![
                    ("origin", fl(vec![("X", tx(ft(f[0]))), ("Y", tx(ft(f[1]))), ("Z", tx(ft(f[2])))])),
                    ("direction", fl(vec![("X", tx(ft(f[3]))), ("Y", tx(ft(f[4]))), ("Z", tx(ft(f[5])))])),
                ]));
            }
            same("Ray", out)
        }
        0x09 => {
            let b = c.take(n)?;
            same("Faces", b.iter().map(|x| fl(vec![("faces", tx(x.to_string()))])).collect())
        }
        0x0A => {
            let b = c.take(n)?;
            same("Axes", b.iter().map(|x| fl(vec![("axes", tx(x.to_string()))])).collect())
        }
        0x0B => {
            let v = c.u32s(n)?;
            same("BrickColor", v.iter().map(|x| tx(x.to_string())).collect())
        }
        0x0C => {
            let r = c.f32s(n)?;
            let g = c.f32s(n)?;
            let b = c.f32s(n)?;
            same(
                "Color3",
                (0..n)
                    .map(|i| fl(vec![("R", tx(ft(r[i]))), ("G", tx(ft(g[i]))), ("B", tx(ft(b[i])))]))
                    .collect(),
            )
        }
        0x0D => {
            let x = c.f32s(n)?;
            let y = c.f32s(n)?;
            same("Vector2", (0..n).map(|i| fl(vec![("X", tx(ft(x[i]))), ("Y", tx(ft(y[i])))])).collect())
        }
        0x0E => {
            let x = c.f32s(n)?;
            let y = c.f32s(n)?;
            let z = c.f32s(n)?;
            same(
                "Vector3",
                (0..n)
                    .map(|i| fl(vec![("X", tx(ft(x[i]))), ("Y", tx(ft(y[i]))), ("Z", tx(ft(z[i])))]))
                    .collect(),
            )
        }
        0x10 => {
            let mut rots: Vec<[f32; 9]> = Vec::new();
            for _ in 0..n {
                let id = c.u8()?;
                let mut m = [0f32; 9];
                if id == 0 {
                    for k in 0..9 {
                        m[k] = c.f32()?;
                    }
                } else {
                    match ROTATIONS.iter().find(|(i, _)| *i == id) {
                        Some((_, r)) => {
                            for k in 0..9 {
                                m[k] = r[k] as f32;
                            }
                        }
                        None => return Err(format!("unknown CFrame rotation id 0x{id:02X}")),
                    }
                }
                rots.push(m);
            }
            let x = c.f32s(n)?;
            let y = c.f32s(n)?;
            let z = c.f32s(n)?;
            same(
                "CoordinateFrame",
                (0..n)
                    .map(|i| {
                        let mut f = vec![("X", tx(ft(x[i]))), ("Y", tx(ft(y[i]))), ("Z", tx(ft(z[i])))];
                        for k in 0..9 {
                            f.push((R_KEYS[k], tx(ft(rots[i][k]))));
                        }
                        fl(f)
                    })
                    .collect(),
            )
        }
        0x12 => {
            let v = c.u32s(n)?;
            same("token", v.iter().map(|x| tx(x.to_string())).collect())
        }
        0x13 => {
            let v = c.referents(n)?;
            same("Ref", v.iter().map(|x| tx(x.to_string())).collect())
        }
        0x14 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let x = c.i16()?;
                let y = c.i16()?;
                let z = c.i16()?;
                out.push(fl(vec![("X", tx(x.to_string())), ("Y", tx(y.to_string())), ("Z", tx(z.to_string()))]));
            }
            same("Vector3int16", out)
        }
        0x15 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let cnt = c.u32()? as usize;
                let mut s = String::new();
                for _ in 0..cnt {
                    for _ in 0..3 {
                        s.push_str(&ft(c.f32()?));
                        s.push(' ');
                    }
                }
                out.push(tx(s));
            }
            same("NumberSequence", out)
        }
        0x16 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let cnt = c.u32()? as usize;
                let mut s = String::new();
                for _ in 0..cnt {
                    for _ in 0..5 {
                        s.push_str(&ft(c.f32()?));
                        s.push(' ');
                    }
                }
                out.push(tx(s));
            }
            same("ColorSequence", out)
        }
        0x17 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let a = c.f32()?;
                let b = c.f32()?;
                out.push(tx(format!("{} {} ", ft(a), ft(b))));
            }
            same("NumberRange", out)
        }
        0x18 => {
            let x0 = c.f32s(n)?;
            let y0 = c.f32s(n)?;
            let x1 = c.f32s(n)?;
            let y1 = c.f32s(n)?;
            same(
                "Rect2D",
                (0..n)
                    .map(|i| {
                        fl(vec![
                            ("min", fl(vec![("X", tx(ft(x0[i]))), ("Y", tx(ft(y0[i])))])),
                            ("max", fl(vec![("X", tx(ft(x1[i]))), ("Y", tx(ft(y1[i])))])),
                        ])
                    })
                    .collect(),
            )
        }
        0x19 => {
            let mut out = Vec::new();
            for _ in 0..n {
                let flag = c.u8()?;
                if flag == 0 {
                    out.push(fl(vec![("CustomPhysics", tx("false"))]));
                } else if flag == 1 {
                    let mut v = [0f32; 5];
                    for k in 0..5 {
                        v[k] = c.f32()?;
                    }
                    out.push(fl(vec![
                        ("CustomPhysics", tx("true")),
                        ("Density", tx(ft(v[0]))),
                        ("Friction", tx(ft(v[1]))),
                        ("Elasticity", tx(ft(v[2]))),
                        ("FrictionWeight", tx(ft(v[3]))),
                        ("ElasticityWeight", tx(ft(v[4]))),
                    ]));
                } else {
                    return Err(format!("unsupported PhysicalProperties flag {flag}"));
                }
            }
            same("PhysicalProperties", out)
        }
        0x1A => {
            let r = c.take(n)?;
            let g = c.take(n)?;
            let b = c.take(n)?;
            same(
                "Color3uint8",
                (0..n)
                    .map(|i| {
                        let v: u32 = 0xFF00_0000 | ((r[i] as u32) << 16) | ((g[i] as u32) << 8) | (b[i] as u32);
                        tx(v.to_string())
                    })
                    .collect(),
            )
        }
        0x1B => {
            let v = c.i64s(n)?;
            same("int64", v.iter().map(|x| tx(x.to_string())).collect())
        }
        0x1C => {
            let idx = c.u32s(n)?;
            let mut out = Vec::new();
            for i in idx {
                match sstr.get(i as usize) {
                    Some(d) => out.push(tx(util::base64_encode(d))),
                    None => return Err("SharedString index out of range".to_string()),
                }
            }
            same("SharedString", out)
        }
        _ => None,
    })
}

struct Raw {
    class: String,
    referent: i32,
    props: Vec<Property>,
}

struct ClassInfo {
    name: String,
    refs: Vec<i32>,
}

struct Builder {
    raws: Vec<Raw>,
    kids: HashMap<i32, Vec<usize>>,
    counter: usize,
    refmap: HashMap<i32, String>,
}

impl Builder {
    fn build(&mut self, idx: usize) -> Instance {
        self.counter += 1;
        let id = format!("instance-{:03}", self.counter);
        let referent = self.raws[idx].referent;
        self.refmap.insert(referent, id.clone());
        let class = self.raws[idx].class.clone();
        let mut inst = Instance::new(&id, &class, &class);
        let props = std::mem::take(&mut self.raws[idx].props);
        for p in props {
            absorb(&mut inst, p);
        }
        let kids = self.kids.get(&referent).cloned().unwrap_or_default();
        for k in kids {
            let c = self.build(k);
            inst.children.push(c);
        }
        inst
    }
}

fn resolve_refs(inst: &mut Instance, map: &HashMap<i32, String>, warnings: &mut Vec<String>) {
    for p in inst.properties.iter_mut() {
        if p.ty != "Ref" {
            continue;
        }
        if let PropValue::Text(t) = &mut p.value {
            let r: i32 = t.trim().parse().unwrap_or(-1);
            if r == -1 {
                *t = "null".to_string();
            } else if let Some(id) = map.get(&r) {
                *t = id.clone();
            } else {
                warnings.push(format!(
                    "{}: dangling reference {} in property {} reset to null",
                    inst.name, r, p.name
                ));
                *t = "null".to_string();
            }
        }
    }
    for c in inst.children.iter_mut() {
        resolve_refs(c, map, warnings);
    }
}

pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    if data.len() < 32 || &data[..MAGIC.len()] != MAGIC {
        return Err("Not a Roblox binary place file (bad header)".to_string());
    }
    let mut c = Cur::new(&data[MAGIC.len()..]);
    let _version = c.take(2)?;
    let _class_count = c.u32()?;
    let _inst_count = c.u32()?;
    c.take(8)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut classes: HashMap<u32, ClassInfo> = HashMap::new();
    let mut raws: Vec<Raw> = Vec::new();
    let mut by_ref: HashMap<i32, usize> = HashMap::new();
    let mut sstr: Vec<Vec<u8>> = Vec::new();
    let mut parents: Vec<(i32, i32)> = Vec::new();

    loop {
        if c.p >= c.d.len() {
            break;
        }
        let name = c.take(4)?;
        let comp = c.u32()? as usize;
        let uncomp = c.u32()? as usize;
        let _reserved = c.u32()?;
        let tag = String::from_utf8_lossy(name).trim_end_matches('\0').to_string();
        let payload: Vec<u8> = if comp == 0 {
            c.take(uncomp)?.to_vec()
        } else {
            let raw = c.take(comp)?;
            lz4::decompress(raw, uncomp)
                .map_err(|e| format!("Failed to decompress binary chunk '{tag}': {e}"))?
        };
        let mut p = Cur::new(&payload);
        match tag.as_str() {
            "META" => {}
            "SSTR" => {
                let _ver = p.u32()?;
                let count = p.u32()? as usize;
                for _ in 0..count {
                    p.take(16)?;
                    sstr.push(p.str_bytes()?.to_vec());
                }
            }
            "INST" => {
                let cid = p.u32()?;
                let cname = String::from_utf8_lossy(p.str_bytes()?).into_owned();
                let svc = p.u8()?;
                let n = p.u32()? as usize;
                let refs = p.referents(n)?;
                if svc == 1 {
                    let _ = p.take(n);
                }
                for &r in &refs {
                    by_ref.insert(r, raws.len());
                    raws.push(Raw { class: cname.clone(), referent: r, props: Vec::new() });
                }
                classes.insert(cid, ClassInfo { name: cname, refs });
            }
            "PROP" => {
                let cid = p.u32()?;
                let pname = String::from_utf8_lossy(p.str_bytes()?).into_owned();
                let tid = p.u8()?;
                let class = match classes.get(&cid) {
                    Some(c) => c,
                    None => {
                        warnings.push(format!("property '{pname}' refers to unknown class id {cid}"));
                        continue;
                    }
                };
                match decode_values(&mut p, tid, class.refs.len(), &pname, &sstr) {
                    Ok(Some(vals)) => {
                        for (i, (ty, v)) in vals.into_iter().enumerate() {
                            if let Some(&idx) = by_ref.get(&class.refs[i]) {
                                raws[idx].props.push(Property { name: pname.clone(), ty, value: v });
                            }
                        }
                    }
                    Ok(None) => warnings.push(format!(
                        "{}.{}: unsupported binary property type 0x{:02X} was skipped",
                        class.name, pname, tid
                    )),
                    Err(e) => warnings.push(format!("{}.{}: {} (property skipped)", class.name, pname, e)),
                }
            }
            "PRNT" => {
                let _ver = p.u8()?;
                let n = p.u32()? as usize;
                let kids = p.referents(n)?;
                let pars = p.referents(n)?;
                for i in 0..n {
                    parents.push((kids[i], pars[i]));
                }
            }
            "END" => break,
            other => warnings.push(format!("unknown chunk '{other}' ignored")),
        }
    }

    let mut kids: HashMap<i32, Vec<usize>> = HashMap::new();
    let mut root_idx: Vec<usize> = Vec::new();
    for (child, parent) in &parents {
        if let Some(&ci) = by_ref.get(child) {
            if *parent == -1 || !by_ref.contains_key(parent) {
                root_idx.push(ci);
            } else {
                kids.entry(*parent).or_default().push(ci);
            }
        }
    }
    let mut b = Builder { raws, kids, counter: 0, refmap: HashMap::new() };
    let mut roots = Vec::new();
    for idx in root_idx {
        roots.push(b.build(idx));
    }
    for r in roots.iter_mut() {
        resolve_refs(r, &b.refmap, &mut warnings);
    }
    Ok(Decoded { roots, warnings })
}

// =============================================================== writing

pub struct Encoded {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_str(b: &mut Vec<u8>, s: &[u8]) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s);
}

fn put_interleaved(b: &mut Vec<u8>, vals: &[u64], w: usize) {
    for lane in 0..w {
        let shift = 8 * (w - 1 - lane);
        for v in vals {
            b.push(((*v >> shift) & 0xFF) as u8);
        }
    }
}

fn put_i32s(b: &mut Vec<u8>, v: &[i32]) {
    let z: Vec<u64> = v.iter().map(|&n| (((n << 1) ^ (n >> 31)) as u32) as u64).collect();
    put_interleaved(b, &z, 4);
}

fn put_u32s(b: &mut Vec<u8>, v: &[u32]) {
    let z: Vec<u64> = v.iter().map(|&n| n as u64).collect();
    put_interleaved(b, &z, 4);
}

fn put_f32s(b: &mut Vec<u8>, v: &[f32]) {
    let z: Vec<u64> = v
        .iter()
        .map(|f| {
            let bits = f.to_bits();
            ((bits << 1) | (bits >> 31)) as u64
        })
        .collect();
    put_interleaved(b, &z, 4);
}

fn put_i64s(b: &mut Vec<u8>, v: &[i64]) {
    let z: Vec<u64> = v.iter().map(|&n| ((n << 1) ^ (n >> 63)) as u64).collect();
    put_interleaved(b, &z, 8);
}

fn put_referents(b: &mut Vec<u8>, v: &[i32]) {
    let mut prev = 0i32;
    let mut deltas = Vec::with_capacity(v.len());
    for &r in v {
        deltas.push(r.wrapping_sub(prev));
        prev = r;
    }
    put_i32s(b, &deltas);
}

fn write_chunk(out: &mut Vec<u8>, name: &[u8; 4], payload: &[u8], compress: bool) {
    out.extend_from_slice(name);
    let mut compressed: Option<Vec<u8>> = None;
    if compress && !payload.is_empty() {
        let c = lz4::compress(payload);
        if c.len() < payload.len() {
            compressed = Some(c);
        }
    }
    match compressed {
        Some(c) => {
            put_u32(out, c.len() as u32);
            put_u32(out, payload.len() as u32);
            put_u32(out, 0);
            out.extend_from_slice(&c);
        }
        None => {
            put_u32(out, 0);
            put_u32(out, payload.len() as u32);
            put_u32(out, 0);
            out.extend_from_slice(payload);
        }
    }
}

// --- value helpers for encoding

fn fd<'a>(v: &'a PropValue, k: &str) -> Result<&'a PropValue, String> {
    match v {
        PropValue::Fields(f) => f
            .iter()
            .find(|(n, _)| n.as_str() == k)
            .map(|(_, x)| x)
            .ok_or_else(|| format!("missing field '{k}'")),
        _ => Err(format!("expected a value with field '{k}'")),
    }
}

fn tv(v: &PropValue) -> Result<&str, String> {
    match v {
        PropValue::Text(t) => Ok(t.as_str()),
        _ => Err("expected a scalar value".to_string()),
    }
}

fn fx<'a>(v: &'a PropValue, k: &str) -> Result<&'a str, String> {
    tv(fd(v, k)?)
}

fn pf(s: &str) -> Result<f32, String> {
    s.trim().parse::<f32>().map_err(|_| format!("invalid number '{s}'"))
}

fn pi(s: &str) -> Result<i32, String> {
    s.trim().parse::<i32>().map_err(|_| format!("invalid integer '{s}'"))
}

fn pu(s: &str) -> Result<u32, String> {
    s.trim().parse::<u32>().map_err(|_| format!("invalid unsigned integer '{s}'"))
}

fn cf(vals: &[&PropValue], k: &str) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(vals.len());
    for v in vals {
        out.push(pf(fx(v, k)?)?);
    }
    Ok(out)
}

fn cf2(vals: &[&PropValue], a: &str, k: &str) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(vals.len());
    for v in vals {
        out.push(pf(fx(fd(v, a)?, k)?)?);
    }
    Ok(out)
}

fn ci(vals: &[&PropValue], k: &str) -> Result<Vec<i32>, String> {
    let mut out = Vec::with_capacity(vals.len());
    for v in vals {
        out.push(pi(fx(v, k)?)?);
    }
    Ok(out)
}

fn texts<'a>(vals: &[&'a PropValue]) -> Result<Vec<&'a str>, String> {
    let mut out = Vec::with_capacity(vals.len());
    for v in vals {
        out.push(tv(v)?);
    }
    Ok(out)
}

fn numbers(s: &str) -> Result<Vec<f32>, String> {
    s.split_whitespace().map(pf).collect()
}

#[derive(Default)]
struct SharedTable {
    items: Vec<Vec<u8>>,
    map: HashMap<Vec<u8>, u32>,
}

impl SharedTable {
    fn add(&mut self, data: Vec<u8>) -> u32 {
        if let Some(&i) = self.map.get(&data) {
            return i;
        }
        let i = self.items.len() as u32;
        self.map.insert(data.clone(), i);
        self.items.push(data);
        i
    }
}

fn encode_values(
    ty: &str,
    vals: &[&PropValue],
    ids: &HashMap<String, i32>,
    sstr: &mut SharedTable,
) -> Result<Option<(u8, Vec<u8>)>, String> {
    let n = vals.len();
    let mut b: Vec<u8> = Vec::new();
    let tid: u8 = match ty {
        "string" | "ProtectedString" | "BinaryString" | "Content" => {
            for v in vals {
                let bytes: Vec<u8> = match (ty, v) {
                    ("BinaryString", PropValue::Text(t)) => {
                        util::base64_decode(t.trim()).unwrap_or_else(|| t.as_bytes().to_vec())
                    }
                    ("Content", PropValue::Fields(f)) => f
                        .iter()
                        .find(|(k, _)| k.as_str() == "url")
                        .and_then(|(_, x)| if let PropValue::Text(t) = x { Some(t.as_bytes().to_vec()) } else { None })
                        .unwrap_or_default(),
                    (_, PropValue::Text(t)) => t.as_bytes().to_vec(),
                    _ => return Err(format!("{ty} value must be text")),
                };
                put_str(&mut b, &bytes);
            }
            0x01
        }
        "bool" => {
            for t in texts(vals)? {
                b.push(if t.trim() == "true" { 1 } else { 0 });
            }
            0x02
        }
        "int" => {
            let v: Result<Vec<i32>, String> = texts(vals)?.into_iter().map(pi).collect();
            put_i32s(&mut b, &v?);
            0x03
        }
        "float" => {
            let v: Result<Vec<f32>, String> = texts(vals)?.into_iter().map(pf).collect();
            put_f32s(&mut b, &v?);
            0x04
        }
        "double" => {
            for t in texts(vals)? {
                let f = t.trim().parse::<f64>().map_err(|_| format!("invalid number '{t}'"))?;
                b.extend_from_slice(&f.to_le_bytes());
            }
            0x05
        }
        "UDim" => {
            put_f32s(&mut b, &cf(vals, "S")?);
            put_i32s(&mut b, &ci(vals, "O")?);
            0x06
        }
        "UDim2" => {
            put_f32s(&mut b, &cf(vals, "XS")?);
            put_f32s(&mut b, &cf(vals, "YS")?);
            put_i32s(&mut b, &ci(vals, "XO")?);
            put_i32s(&mut b, &ci(vals, "YO")?);
            0x07
        }
        "Ray" => {
            for v in vals {
                for part in ["origin", "direction"] {
                    for k in ["X", "Y", "Z"] {
                        let f = pf(fx(fd(v, part)?, k)?)?;
                        b.extend_from_slice(&f.to_le_bytes());
                    }
                }
            }
            0x08
        }
        "Faces" => {
            for v in vals {
                let x = fx(v, "faces")?.trim().parse::<u8>().map_err(|_| "invalid Faces value".to_string())?;
                b.push(x);
            }
            0x09
        }
        "Axes" => {
            for v in vals {
                let x = fx(v, "axes")?.trim().parse::<u8>().map_err(|_| "invalid Axes value".to_string())?;
                b.push(x);
            }
            0x0A
        }
        "BrickColor" => {
            let v: Result<Vec<u32>, String> = texts(vals)?.into_iter().map(pu).collect();
            put_u32s(&mut b, &v?);
            0x0B
        }
        "Color3" => {
            let mut r = Vec::with_capacity(n);
            let mut g = Vec::with_capacity(n);
            let mut bl = Vec::with_capacity(n);
            for v in vals {
                match v {
                    PropValue::Text(t) => {
                        let u = pu(t)?;
                        r.push(((u >> 16) & 255) as f32 / 255.0);
                        g.push(((u >> 8) & 255) as f32 / 255.0);
                        bl.push((u & 255) as f32 / 255.0);
                    }
                    _ => {
                        r.push(pf(fx(v, "R")?)?);
                        g.push(pf(fx(v, "G")?)?);
                        bl.push(pf(fx(v, "B")?)?);
                    }
                }
            }
            put_f32s(&mut b, &r);
            put_f32s(&mut b, &g);
            put_f32s(&mut b, &bl);
            0x0C
        }
        "Vector2" => {
            put_f32s(&mut b, &cf(vals, "X")?);
            put_f32s(&mut b, &cf(vals, "Y")?);
            0x0D
        }
        "Vector3" => {
            put_f32s(&mut b, &cf(vals, "X")?);
            put_f32s(&mut b, &cf(vals, "Y")?);
            put_f32s(&mut b, &cf(vals, "Z")?);
            0x0E
        }
        "CoordinateFrame" => {
            for v in vals {
                b.push(0);
                for k in R_KEYS {
                    let f = pf(fx(v, k)?)?;
                    b.extend_from_slice(&f.to_le_bytes());
                }
            }
            put_f32s(&mut b, &cf(vals, "X")?);
            put_f32s(&mut b, &cf(vals, "Y")?);
            put_f32s(&mut b, &cf(vals, "Z")?);
            0x10
        }
        "token" => {
            let v: Result<Vec<u32>, String> = texts(vals)?.into_iter().map(pu).collect();
            put_u32s(&mut b, &v?);
            0x12
        }
        "Ref" => {
            let mut v = Vec::with_capacity(n);
            for t in texts(vals)? {
                let t = t.trim();
                if t == "null" {
                    v.push(-1);
                } else {
                    match ids.get(t) {
                        Some(r) => v.push(*r),
                        None => return Err(format!("Missing instance reference '{t}'")),
                    }
                }
            }
            put_referents(&mut b, &v);
            0x13
        }
        "Vector3int16" => {
            for v in vals {
                for k in ["X", "Y", "Z"] {
                    let x = fx(v, k)?.trim().parse::<i16>().map_err(|_| "invalid Vector3int16 value".to_string())?;
                    b.extend_from_slice(&x.to_le_bytes());
                }
            }
            0x14
        }
        "NumberSequence" | "ColorSequence" => {
            let stride = if ty == "NumberSequence" { 3 } else { 5 };
            for t in texts(vals)? {
                let nums = numbers(t)?;
                if nums.len() % stride != 0 {
                    return Err(format!("{ty} must contain groups of {stride} numbers"));
                }
                put_u32(&mut b, (nums.len() / stride) as u32);
                for f in nums {
                    b.extend_from_slice(&f.to_le_bytes());
                }
            }
            if ty == "NumberSequence" { 0x15 } else { 0x16 }
        }
        "NumberRange" => {
            for t in texts(vals)? {
                let nums = numbers(t)?;
                if nums.len() < 2 {
                    return Err("NumberRange needs two numbers".to_string());
                }
                b.extend_from_slice(&nums[0].to_le_bytes());
                b.extend_from_slice(&nums[1].to_le_bytes());
            }
            0x17
        }
        "Rect2D" => {
            put_f32s(&mut b, &cf2(vals, "min", "X")?);
            put_f32s(&mut b, &cf2(vals, "min", "Y")?);
            put_f32s(&mut b, &cf2(vals, "max", "X")?);
            put_f32s(&mut b, &cf2(vals, "max", "Y")?);
            0x18
        }
        "PhysicalProperties" => {
            for v in vals {
                if fx(v, "CustomPhysics")?.trim() == "true" {
                    b.push(1);
                    for k in ["Density", "Friction", "Elasticity", "FrictionWeight", "ElasticityWeight"] {
                        b.extend_from_slice(&pf(fx(v, k)?)?.to_le_bytes());
                    }
                } else {
                    b.push(0);
                }
            }
            0x19
        }
        "Color3uint8" => {
            let mut r = Vec::with_capacity(n);
            let mut g = Vec::with_capacity(n);
            let mut bl = Vec::with_capacity(n);
            for t in texts(vals)? {
                let u = pu(t)?;
                r.push(((u >> 16) & 255) as u8);
                g.push(((u >> 8) & 255) as u8);
                bl.push((u & 255) as u8);
            }
            b.extend_from_slice(&r);
            b.extend_from_slice(&g);
            b.extend_from_slice(&bl);
            0x1A
        }
        "int64" => {
            let mut v = Vec::with_capacity(n);
            for t in texts(vals)? {
                v.push(t.trim().parse::<i64>().map_err(|_| format!("invalid integer '{t}'"))?);
            }
            put_i64s(&mut b, &v);
            0x1B
        }
        "SharedString" => {
            let mut idx = Vec::with_capacity(n);
            for t in texts(vals)? {
                let data: String = t.chars().filter(|c| !c.is_whitespace()).collect();
                let bytes = util::base64_decode(&data).unwrap_or_default();
                idx.push(sstr.add(bytes));
            }
            put_u32s(&mut b, &idx);
            0x1C
        }
        _ => return Ok(None),
    };
    Ok(Some((tid, b)))
}

struct Flat<'a> {
    inst: &'a Instance,
    parent: i32,
}

fn walk<'a>(i: &'a Instance, parent: i32, flat: &mut Vec<Flat<'a>>, idmap: &mut HashMap<String, i32>) {
    let me = flat.len() as i32;
    idmap.insert(i.id.clone(), me);
    flat.push(Flat { inst: i, parent });
    for c in &i.children {
        walk(c, me, flat, idmap);
    }
}

struct Group {
    class: String,
    root: bool,
    sig: Vec<(String, String)>,
    members: Vec<i32>,
    props: Vec<Vec<Property>>,
}

pub fn encode(roots: &[Instance]) -> Result<Encoded, String> {
    let mut flat: Vec<Flat> = Vec::new();
    let mut idmap: HashMap<String, i32> = HashMap::new();
    for r in roots {
        walk(r, -1, &mut flat, &mut idmap);
    }

    // Instances of one class are grouped by identical property sets, because every
    // instance in an INST group must carry a value for every property in its PROP chunks.
    let mut groups: Vec<Group> = Vec::new();
    let mut gindex: HashMap<(String, bool, Vec<(String, String)>), usize> = HashMap::new();
    for (i, f) in flat.iter().enumerate() {
        let mut props: Vec<Property> = Vec::new();
        props.push(Property {
            name: "Name".to_string(),
            ty: "string".to_string(),
            value: PropValue::Text(f.inst.name.clone()),
        });
        props.extend(f.inst.properties.iter().cloned());
        if let Some(s) = &f.inst.source {
            props.push(Property {
                name: "Source".to_string(),
                ty: "ProtectedString".to_string(),
                value: PropValue::Text(s.clone()),
            });
        }
        if !f.inst.tags.is_empty() {
            let joined = f.inst.tags.join("\0");
            props.push(Property {
                name: "Tags".to_string(),
                ty: "BinaryString".to_string(),
                value: PropValue::Text(util::base64_encode(joined.as_bytes())),
            });
        }
        props.sort_by(|a, b| a.name.cmp(&b.name));
        let sig: Vec<(String, String)> = props.iter().map(|p| (p.name.clone(), p.ty.clone())).collect();
        let root = f.parent < 0;
        let key = (f.inst.class.clone(), root, sig.clone());
        let gi = match gindex.get(&key) {
            Some(&g) => g,
            None => {
                groups.push(Group {
                    class: f.inst.class.clone(),
                    root,
                    sig,
                    members: Vec::new(),
                    props: Vec::new(),
                });
                let g = groups.len() - 1;
                gindex.insert(key, g);
                g
            }
        };
        groups[gi].members.push(i as i32);
        groups[gi].props.push(props);
    }

    let mut sstr = SharedTable::default();
    let mut warnings: Vec<String> = Vec::new();
    let mut prop_chunks: Vec<Vec<u8>> = Vec::new();
    for (gid, g) in groups.iter().enumerate() {
        for (j, (pname, pty)) in g.sig.iter().enumerate() {
            let vals: Vec<&PropValue> = g.props.iter().map(|ps| &ps[j].value).collect();
            let res = encode_values(pty, &vals, &idmap, &mut sstr)
                .map_err(|e| format!("{}.{}: {e}", g.class, pname))?;
            match res {
                Some((tid, data)) => {
                    let mut b = Vec::new();
                    put_u32(&mut b, gid as u32);
                    put_str(&mut b, pname.as_bytes());
                    b.push(tid);
                    b.extend_from_slice(&data);
                    prop_chunks.push(b);
                }
                None => warnings.push(format!(
                    "{}.{}: property type '{}' is not supported in binary output and was skipped",
                    g.class, pname, pty
                )),
            }
        }
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(groups.len() as i32).to_le_bytes());
    out.extend_from_slice(&(flat.len() as i32).to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);

    let mut meta = Vec::new();
    put_u32(&mut meta, 1);
    put_str(&mut meta, b"ExplicitAutoJoints");
    put_str(&mut meta, b"true");
    write_chunk(&mut out, b"META", &meta, true);

    if !sstr.items.is_empty() {
        let mut b = Vec::new();
        put_u32(&mut b, 0);
        put_u32(&mut b, sstr.items.len() as u32);
        for d in &sstr.items {
            b.extend_from_slice(&util::hash16(d));
            put_str(&mut b, d);
        }
        write_chunk(&mut out, b"SSTR", &b, true);
    }

    for (gid, g) in groups.iter().enumerate() {
        let mut b = Vec::new();
        put_u32(&mut b, gid as u32);
        put_str(&mut b, g.class.as_bytes());
        b.push(if g.root { 1 } else { 0 });
        put_u32(&mut b, g.members.len() as u32);
        put_referents(&mut b, &g.members);
        if g.root {
            b.extend(std::iter::repeat(1u8).take(g.members.len()));
        }
        write_chunk(&mut out, b"INST", &b, true);
    }

    for b in &prop_chunks {
        write_chunk(&mut out, b"PROP", b, true);
    }

    {
        let n = flat.len();
        let mut b = vec![0u8];
        put_u32(&mut b, n as u32);
        let kids: Vec<i32> = (0..n as i32).collect();
        let pars: Vec<i32> = flat.iter().map(|f| f.parent).collect();
        put_referents(&mut b, &kids);
        put_referents(&mut b, &pars);
        write_chunk(&mut out, b"PRNT", &b, true);
    }

    write_chunk(&mut out, b"END\0", b"</roblox>", false);
    Ok(Encoded { bytes: out, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbxlx;

    const SAMPLE: &str = "<roblox version=\"4\"><Item class=\"Workspace\" referent=\"RBX1\"><Properties><string name=\"Name\">Workspace</string></Properties><Item class=\"Part\" referent=\"RBX2\"><Properties><string name=\"Name\">P</string><bool name=\"Anchored\">true</bool><float name=\"Transparency\">0.5</float><token name=\"Material\">256</token><Color3uint8 name=\"Color3uint8\">4288914085</Color3uint8><Vector3 name=\"size\"><X>1</X><Y>2</Y><Z>3</Z></Vector3><CoordinateFrame name=\"CFrame\"><X>0</X><Y>-0.5</Y><Z>0</Z><R00>1</R00><R01>0</R01><R02>0</R02><R10>0</R10><R11>1</R11><R12>0</R12><R20>0</R20><R21>0</R21><R22>1</R22></CoordinateFrame><Content name=\"MeshId\"><url>rbxassetid://1</url></Content><Ref name=\"PrimaryPart\">RBX2</Ref></Properties></Item><Item class=\"Script\" referent=\"RBX3\"><Properties><string name=\"Name\">S</string><ProtectedString name=\"Source\"><![CDATA[print(1)]]></ProtectedString></Properties></Item></Item></roblox>";

    #[test]
    fn binary_round_trip() {
        let d = rbxlx::decode(SAMPLE).unwrap();
        let e = encode(&d.roots).unwrap();
        assert!(e.bytes.starts_with(b"<roblox!"));
        let back = decode(&e.bytes).unwrap();
        assert_eq!(back.roots.len(), 1);
        assert_eq!(back.roots[0].count(), 3);
        let part = &back.roots[0].children[0];
        assert_eq!(part.name, "P");
        assert_eq!(part.properties.len(), d.roots[0].children[0].properties.len());
        assert!(part.properties.iter().any(|p| p.name == "size" && p.ty == "Vector3"));
        let r = part.properties.iter().find(|p| p.name == "PrimaryPart").unwrap();
        match &r.value {
            PropValue::Text(t) => assert_eq!(t, &part.id),
            _ => panic!("ref should be text"),
        }
        assert_eq!(back.roots[0].children[1].source.as_deref(), Some("print(1)"));
    }

    #[test]
    fn integer_lanes_round_trip() {
        let vals = vec![0, 1, -1, 300, -70000, i32::MAX, i32::MIN];
        let mut b = Vec::new();
        put_i32s(&mut b, &vals);
        let mut c = Cur::new(&b);
        assert_eq!(c.i32s(vals.len()).unwrap(), vals);
        let fs = vec![0.0f32, 1.5, -2.25, 1e10];
        let mut b = Vec::new();
        put_f32s(&mut b, &fs);
        let mut c = Cur::new(&b);
        assert_eq!(c.f32s(fs.len()).unwrap(), fs);
    }
}
