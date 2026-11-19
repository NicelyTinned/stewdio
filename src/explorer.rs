//! `stewdio explorer`: an interactive instance explorer / properties / script editor.
use clap::Args;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crossterm::style::Color;
use crossterm::terminal;
use std::collections::{HashMap, HashSet};
use std::io;

use crate::api::{self, Api};
use crate::editor::{Editor, EditorEvent};
use crate::error::*;
use crate::model::*;
use crate::project::Project;
use crate::tui::{self, st, Screen, Style, Term};
use crate::{fsproj, ui, util};

#[derive(Args)]
pub struct ExplorerArgs {}

// ------------------------------------------------------------------ tree helpers

fn get<'a>(roots: &'a [Instance], path: &[usize]) -> Option<&'a Instance> {
    let (first, rest) = path.split_first()?;
    let mut cur = roots.get(*first)?;
    for i in rest {
        cur = cur.children.get(*i)?;
    }
    Some(cur)
}

fn get_mut<'a>(roots: &'a mut [Instance], path: &[usize]) -> Option<&'a mut Instance> {
    let (first, rest) = path.split_first()?;
    let mut cur = roots.get_mut(*first)?;
    for i in rest {
        cur = cur.children.get_mut(*i)?;
    }
    Some(cur)
}

fn children_mut<'a>(roots: &'a mut Vec<Instance>, parent: &[usize]) -> Option<&'a mut Vec<Instance>> {
    if parent.is_empty() {
        return Some(roots);
    }
    get_mut(roots, parent).map(|i| &mut i.children)
}

fn remove_at(roots: &mut Vec<Instance>, path: &[usize]) -> Option<Instance> {
    let (last, parent) = path.split_last()?;
    let list = children_mut(roots, parent)?;
    if *last < list.len() {
        Some(list.remove(*last))
    } else {
        None
    }
}

fn find_path(roots: &[Instance], id: &str) -> Option<Vec<usize>> {
    fn go(list: &[Instance], id: &str, path: &mut Vec<usize>) -> bool {
        for (i, inst) in list.iter().enumerate() {
            path.push(i);
            if inst.id == id || go(&inst.children, id, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    if go(roots, id, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn collect_ids(inst: &Instance, out: &mut HashSet<String>) {
    out.insert(inst.id.clone());
    for c in &inst.children {
        collect_ids(c, out);
    }
}

fn all_ids(roots: &[Instance]) -> HashSet<String> {
    let mut s = HashSet::new();
    for r in roots {
        collect_ids(r, &mut s);
    }
    s
}

fn null_refs(inst: &mut Instance, dead: &HashSet<String>) {
    for p in inst.properties.iter_mut() {
        if p.ty == "Ref" {
            if let PropValue::Text(t) = &mut p.value {
                if dead.contains(t.as_str()) {
                    *t = "null".to_string();
                }
            }
        }
    }
    for c in inst.children.iter_mut() {
        null_refs(c, dead);
    }
}

fn collect_map(inst: &Instance, next: &mut usize, map: &mut HashMap<String, String>) {
    *next += 1;
    map.insert(inst.id.clone(), format!("instance-{:03}", *next));
    for c in &inst.children {
        collect_map(c, next, map);
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

fn max_numeric_id(roots: &[Instance]) -> usize {
    fn go(i: &Instance, m: &mut usize) {
        if let Some(n) = i.id.strip_prefix("instance-").and_then(|s| s.parse::<usize>().ok()) {
            *m = (*m).max(n);
        }
        for c in &i.children {
            go(c, m);
        }
    }
    let mut m = 0;
    for r in roots {
        go(r, &mut m);
    }
    m
}

fn contains_path(ancestor: &[usize], p: &[usize]) -> bool {
    p.len() >= ancestor.len() && &p[..ancestor.len()] == ancestor
}

// ------------------------------------------------------------------ model of the UI

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Explorer,
    Props,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Edit,
    Rename,
    Cut,
    Copy,
    Paste,
    Add,
    Insert,
    Delete,
}

struct Row {
    path: Vec<usize>,
    depth: usize,
    has_children: bool,
    open: bool,
    id: String,
    name: String,
    class: String,
}

struct Clip {
    items: Vec<Instance>,
    cut: bool,
}

struct Menu {
    items: Vec<(Action, &'static str)>,
    sel: usize,
}

enum Purpose {
    Rename(String),
    EditProp(String, String),
    Insert(Option<String>),
    Add(String),
    Tags(String),
    Search,
}

struct Input {
    title: String,
    buf: Vec<char>,
    cur: usize,
    purpose: Purpose,
}

enum ConfirmKind {
    Delete(String),
    Quit,
}

enum Mode {
    Normal,
    Menu(Menu),
    Input(Input),
    Confirm(String, ConfirmKind),
    Editor(Box<Editor>, String),
}

#[derive(Clone)]
enum PKind {
    Name,
    Class,
    Id,
    Tags,
    Source,
    Prop(String),
}

struct PRow {
    key: String,
    val: String,
    kind: PKind,
}

fn summarize(v: &PropValue) -> String {
    match v {
        PropValue::Text(t) => t.replace('\n', "⏎"),
        PropValue::Fields(f) => f
            .iter()
            .map(|(k, x)| match x {
                PropValue::Text(t) => format!("{k}={t}"),
                PropValue::Fields(_) => format!("{k}={{…}}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn prop_rows(inst: &Instance) -> Vec<PRow> {
    let mut rows = vec![
        PRow { key: "Name".into(), val: inst.name.clone(), kind: PKind::Name },
        PRow { key: "ClassName".into(), val: inst.class.clone(), kind: PKind::Class },
        PRow { key: "$id".into(), val: inst.id.clone(), kind: PKind::Id },
        PRow { key: "Tags".into(), val: inst.tags.join(", "), kind: PKind::Tags },
    ];
    if is_script_class(&inst.class) {
        let n = inst.source.as_ref().map(|s| s.lines().count()).unwrap_or(0);
        rows.push(PRow { key: "Source".into(), val: format!("({n} lines) Enter to open the editor"), kind: PKind::Source });
    }
    for p in &inst.properties {
        rows.push(PRow {
            key: p.name.clone(),
            val: format!("{}  {}", summarize(&p.value), format!("<{}>", p.ty)),
            kind: PKind::Prop(p.name.clone()),
        });
    }
    rows
}

fn apply_prop_edit(inst: &mut Instance, name: &str, input: &str, ids: &HashSet<String>) -> Result<(), String> {
    let p = inst
        .properties
        .iter_mut()
        .find(|p| p.name == name)
        .ok_or_else(|| "property not found".to_string())?;
    let ty = p.ty.clone();
    match &mut p.value {
        PropValue::Text(t) => {
            let v = input.trim();
            match ty.as_str() {
                "bool" => {
                    if v != "true" && v != "false" {
                        return Err("expected true or false".into());
                    }
                }
                "int" | "int64" | "token" | "BrickColor" => {
                    v.parse::<i64>().map_err(|_| "expected an integer".to_string())?;
                }
                "float" | "double" => {
                    v.parse::<f64>().map_err(|_| "expected a number".to_string())?;
                }
                "Color3uint8" => {
                    v.parse::<u32>().map_err(|_| "expected an unsigned integer".to_string())?;
                }
                "Ref" => {
                    if v != "null" && !ids.contains(v) {
                        return Err(format!("no instance with $id '{v}' (or use null)"));
                    }
                }
                _ => {}
            }
            *t = if matches!(ty.as_str(), "string" | "ProtectedString" | "Content" | "BinaryString" | "NumberSequence" | "ColorSequence" | "NumberRange") {
                input.to_string()
            } else {
                v.to_string()
            };
            Ok(())
        }
        PropValue::Fields(fields) => {
            let mut parsed: Vec<(String, String)> = Vec::new();
            for part in input.split_whitespace() {
                let (k, v) = part.split_once('=').ok_or_else(|| format!("expected key=value, got '{part}'"))?;
                parsed.push((k.to_string(), v.to_string()));
            }
            for (k, v) in parsed {
                match fields.iter_mut().find(|(n, _)| n.as_str() == k) {
                    Some((_, PropValue::Text(t))) => *t = v,
                    Some(_) => return Err(format!("'{k}' is nested; edit Instance.data.hjson instead")),
                    None => return Err(format!("unknown field '{k}'")),
                }
            }
            Ok(())
        }
    }
}

fn editable_text(v: &PropValue) -> Option<String> {
    match v {
        PropValue::Text(t) => Some(t.clone()),
        PropValue::Fields(f) => {
            let mut parts = Vec::new();
            for (k, x) in f {
                match x {
                    PropValue::Text(t) => parts.push(format!("{k}={t}")),
                    PropValue::Fields(_) => return None,
                }
            }
            Some(parts.join(" "))
        }
    }
}

struct App {
    project: Project,
    roots: Vec<Instance>,
    rows: Vec<Row>,
    open: HashSet<String>,
    sel: usize,
    scroll: usize,
    psel: usize,
    pscroll: usize,
    focus: Focus,
    mode: Mode,
    clip: Option<Clip>,
    undo: Vec<Vec<Instance>>,
    dirty: bool,
    status: String,
    status_err: bool,
    api: Option<Api>,
    stale: Option<String>,
    search: String,
    next_id: usize,
}

fn layout(h: usize) -> (usize, usize) {
    let usable = h.saturating_sub(3);
    let ex = usable / 2;
    (ex, usable - ex)
}

fn class_style(class: &str) -> Style {
    if is_script_class(class) {
        st(Color::Green, Color::Reset)
    } else if SERVICES.contains(&class) {
        st(Color::Cyan, Color::Reset)
    } else if class == "Folder" || class == "Model" {
        st(Color::Yellow, Color::Reset)
    } else {
        tui::NORMAL
    }
}

fn menu_geom(w: usize, h: usize, items: &[(Action, &'static str)]) -> (usize, usize, usize, usize) {
    let bw = 26usize.min(w);
    let bh = (items.len() + 2).min(h);
    ((w - bw) / 2, (h.saturating_sub(bh)) / 2, bw, bh)
}

impl App {
    fn new(project: Project, roots: Vec<Instance>, api: Option<Api>) -> App {
        let stale = api::stale_hint(api.as_ref());
        let next_id = max_numeric_id(&roots);
        let mut open = HashSet::new();
        for r in &roots {
            open.insert(r.id.clone());
        }
        let mut a = App {
            project,
            roots,
            rows: Vec::new(),
            open,
            sel: 0,
            scroll: 0,
            psel: 0,
            pscroll: 0,
            focus: Focus::Explorer,
            mode: Mode::Normal,
            clip: None,
            undo: Vec::new(),
            dirty: false,
            status: String::new(),
            status_err: false,
            api,
            stale,
            search: String::new(),
            next_id,
        };
        a.rebuild();
        a
    }

    fn walk(list: &[Instance], path: &mut Vec<usize>, depth: usize, open: &HashSet<String>, rows: &mut Vec<Row>) {
        for (i, inst) in list.iter().enumerate() {
            path.push(i);
            let is_open = open.contains(&inst.id);
            rows.push(Row {
                path: path.clone(),
                depth,
                has_children: !inst.children.is_empty(),
                open: is_open,
                id: inst.id.clone(),
                name: inst.name.clone(),
                class: inst.class.clone(),
            });
            if is_open && !inst.children.is_empty() {
                Self::walk(&inst.children, path, depth + 1, open, rows);
            }
            path.pop();
        }
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        Self::walk(&self.roots, &mut Vec::new(), 0, &self.open, &mut rows);
        self.rows = rows;
        if self.rows.is_empty() {
            self.sel = 0;
        } else if self.sel >= self.rows.len() {
            self.sel = self.rows.len() - 1;
        }
    }

    fn select_id(&mut self, id: &str) {
        if let Some(i) = self.rows.iter().position(|r| r.id == id) {
            self.sel = i;
        }
    }

    fn selected(&self) -> Option<&Instance> {
        let r = self.rows.get(self.sel)?;
        get(&self.roots, &r.path)
    }

    fn selected_id(&self) -> Option<String> {
        self.rows.get(self.sel).map(|r| r.id.clone())
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_err = false;
    }

    fn fail(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_err = true;
    }

    fn snapshot(&mut self) {
        self.undo.push(self.roots.clone());
        if self.undo.len() > 40 {
            self.undo.remove(0);
        }
        self.dirty = true;
    }

    fn fresh_id(&mut self) -> String {
        self.next_id += 1;
        format!("instance-{:03}", self.next_id)
    }

    fn clone_fresh(&mut self, inst: &Instance) -> Instance {
        let mut map = HashMap::new();
        let mut next = self.next_id;
        collect_map(inst, &mut next, &mut map);
        self.next_id = next;
        let mut c = inst.clone();
        apply_map(&mut c, &map);
        c
    }

    // ------------------------------------------------------------ actions

    fn menu_items(&self) -> Vec<(Action, &'static str)> {
        let mut v = vec![(Action::Edit, "Edit"), (Action::Rename, "Rename"), (Action::Cut, "Cut"), (Action::Copy, "Copy")];
        if self.clip.is_some() {
            v.push((Action::Paste, "Paste (Ctrl+P)"));
        }
        v.push((Action::Add, "Add (sibling below)"));
        v.push((Action::Insert, "Insert child"));
        v.push((Action::Delete, "Delete"));
        v
    }

    fn open_menu(&mut self) {
        if self.rows.is_empty() {
            self.mode = Mode::Input(Input {
                title: "Insert instance (class name)".into(),
                buf: Vec::new(),
                cur: 0,
                purpose: Purpose::Insert(None),
            });
            return;
        }
        self.mode = Mode::Menu(Menu { items: self.menu_items(), sel: 0 });
    }

    fn prompt(&mut self, title: &str, initial: &str, purpose: Purpose) {
        let buf: Vec<char> = initial.chars().collect();
        let cur = buf.len();
        self.mode = Mode::Input(Input { title: title.to_string(), buf, cur, purpose });
    }

    fn do_action(&mut self, a: Action) {
        let id = match self.selected_id() {
            Some(i) => i,
            None => return,
        };
        let inst = match self.selected() {
            Some(i) => i.clone(),
            None => return,
        };
        match a {
            Action::Edit => {
                if is_script_class(&inst.class) {
                    self.open_editor(&id);
                } else {
                    self.focus = Focus::Props;
                    self.psel = 0;
                    self.pscroll = 0;
                    self.say("Properties: Up/Down select, Enter edits, Esc/Tab returns to the explorer");
                }
            }
            Action::Rename => self.prompt("Rename", &inst.name, Purpose::Rename(id)),
            Action::Copy => {
                self.clip = Some(Clip { items: vec![inst.clone()], cut: false });
                self.say(format!("Copied '{}'", inst.name));
            }
            Action::Cut => {
                self.clip = Some(Clip { items: vec![inst.clone()], cut: true });
                self.say(format!("Cut '{}' (Ctrl+P to paste it)", inst.name));
            }
            Action::Paste => self.paste(),
            Action::Add => self.prompt("Add instance below (class name)", &inst.class, Purpose::Add(id)),
            Action::Insert => self.prompt("Insert child (class name)", "Part", Purpose::Insert(Some(id))),
            Action::Delete => {
                let n = inst.count() - 1;
                let msg = if n > 0 {
                    format!("Delete '{}' and its {} descendant(s)?", inst.name, n)
                } else {
                    format!("Delete '{}'?", inst.name)
                };
                self.mode = Mode::Confirm(msg, ConfirmKind::Delete(id));
            }
        }
    }

    fn open_editor(&mut self, id: &str) {
        if let Some(path) = find_path(&self.roots, id) {
            if let Some(inst) = get(&self.roots, &path) {
                let ed = Editor::new(&inst.name, inst.source.as_deref().unwrap_or(""));
                self.mode = Mode::Editor(Box::new(ed), id.to_string());
            }
        }
    }

    fn delete(&mut self, id: &str) {
        if let Some(path) = find_path(&self.roots, id) {
            self.snapshot();
            if let Some(removed) = remove_at(&mut self.roots, &path) {
                let mut dead = HashSet::new();
                collect_ids(&removed, &mut dead);
                for r in self.roots.iter_mut() {
                    null_refs(r, &dead);
                }
                self.say(format!("Deleted '{}'", removed.name));
            }
            self.rebuild();
        }
    }

    fn paste(&mut self) {
        let clip = match self.clip.take() {
            Some(c) => c,
            None => {
                self.fail("Nothing to paste");
                return;
            }
        };
        let target_path: Vec<usize> = self.rows.get(self.sel).map(|r| r.path.clone()).unwrap_or_default();
        if clip.cut {
            // refuse to move an instance into itself
            let mut bad = false;
            for it in &clip.items {
                if let Some(src) = find_path(&self.roots, &it.id) {
                    if contains_path(&src, &target_path) {
                        bad = true;
                    }
                }
            }
            if bad {
                self.clip = Some(clip);
                self.fail("Cannot paste an instance into itself");
                return;
            }
        }
        self.snapshot();
        let mut pasted: Vec<Instance> = Vec::new();
        for it in &clip.items {
            if clip.cut {
                if let Some(src) = find_path(&self.roots, &it.id) {
                    if let Some(moved) = remove_at(&mut self.roots, &src) {
                        pasted.push(moved);
                    }
                }
            } else {
                let c = self.clone_fresh(it);
                pasted.push(c);
            }
        }
        // the target path may have shifted after removing cut items; locate it again by id
        let target_id = self.rows.get(self.sel).map(|r| r.id.clone());
        let new_path = target_id.and_then(|id| find_path(&self.roots, &id)).unwrap_or_default();
        let n = pasted.len();
        let mut last_id = None;
        if let Some(list) = if new_path.is_empty() {
            Some(&mut self.roots)
        } else {
            get_mut(&mut self.roots, &new_path).map(|i| &mut i.children)
        } {
            for p in pasted {
                last_id = Some(p.id.clone());
                list.push(p);
            }
        }
        if let Some(id) = self.rows.get(self.sel).map(|r| r.id.clone()) {
            self.open.insert(id);
        }
        self.rebuild();
        if let Some(id) = last_id {
            self.select_id(&id);
        }
        if clip.cut {
            self.clip = None;
        } else {
            self.clip = Some(clip);
        }
        self.say(format!("Pasted {n} instance(s)"));
    }

    /// Validate a class name (syntax + the API database when available).
    fn check_class(&mut self, class: &str) -> Option<String> {
        let class = class.trim().to_string();
        if class.is_empty() || !class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.fail("Invalid class name");
            return None;
        }
        let unknown = match &self.api {
            Some(api) if !api.classes.contains_key(&class) => Some(
                util::suggest(&class, api.classes.keys().map(|s| s.as_str()))
                    .map(|s| format!(" (did you mean '{s}'?)"))
                    .unwrap_or_default(),
            ),
            _ => None,
        };
        if let Some(hint) = unknown {
            self.fail(format!("Unknown class '{class}'{hint}"));
            return None;
        }
        Some(class)
    }

    fn insert(&mut self, parent: Option<String>, class: &str) {
        let class = match self.check_class(class) {
            Some(c) => c,
            None => return,
        };
        self.snapshot();
        let id = self.fresh_id();
        let inst = Instance::new(&id, &class, &class);
        match parent.and_then(|p| find_path(&self.roots, &p)) {
            Some(path) => {
                if let Some(p) = get_mut(&mut self.roots, &path) {
                    let pid = p.id.clone();
                    p.children.push(inst);
                    self.open.insert(pid);
                }
            }
            None => self.roots.push(inst),
        }
        self.rebuild();
        self.select_id(&id);
        self.say(format!("Inserted {class}"));
    }

    /// Add a new instance as the next sibling of `below`.
    fn add_below(&mut self, below: String, class: &str) {
        let class = match self.check_class(class) {
            Some(c) => c,
            None => return,
        };
        let path = match find_path(&self.roots, &below) {
            Some(p) => p,
            None => {
                self.fail("The selected instance no longer exists");
                return;
            }
        };
        self.snapshot();
        let id = self.fresh_id();
        let inst = Instance::new(&id, &class, &class);
        if let Some((last, parent)) = path.split_last() {
            if let Some(list) = children_mut(&mut self.roots, parent) {
                let at = (*last + 1).min(list.len());
                list.insert(at, inst);
            }
        }
        self.rebuild();
        self.select_id(&id);
        self.say(format!("Added {class} below the selected instance"));
    }

    fn save(&mut self) {
        let result = fsproj::save_project(&self.project.root, self.project.is_model(), &self.roots);
        match result {
            Ok(backup) => {
                self.dirty = false;
                match backup {
                    Some(b) => self.say(format!("Saved. Previous version backed up in {}", b.display())),
                    None => self.say("Saved"),
                }
            }
            Err(e) => self.fail(format!("Save failed: {e} (see .stewdio/backup)")),
        }
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.roots = prev;
                self.dirty = true;
                self.rebuild();
                self.say("Undone");
            }
            None => self.fail("Nothing to undo"),
        }
    }

    fn search_next(&mut self) {
        if self.search.is_empty() {
            return;
        }
        let needle = self.search.to_lowercase();
        // search the whole tree in display order, opening ancestors as needed
        fn flat(list: &[Instance], out: &mut Vec<(String, String, Vec<String>)>, anc: &mut Vec<String>) {
            for i in list {
                out.push((i.id.clone(), i.name.to_lowercase(), anc.clone()));
                anc.push(i.id.clone());
                flat(&i.children, out, anc);
                anc.pop();
            }
        }
        let mut all = Vec::new();
        flat(&self.roots, &mut all, &mut Vec::new());
        if all.is_empty() {
            return;
        }
        let cur_id = self.selected_id().unwrap_or_default();
        let start = all.iter().position(|(id, _, _)| *id == cur_id).map(|p| p + 1).unwrap_or(0);
        for off in 0..all.len() {
            let (id, name, anc) = &all[(start + off) % all.len()];
            if name.contains(&needle) {
                for a in anc {
                    self.open.insert(a.clone());
                }
                let id = id.clone();
                self.rebuild();
                self.select_id(&id);
                self.say(format!("Found '{}'", self.search));
                return;
            }
        }
        self.fail(format!("No instance named '{}'", self.search));
    }

    fn activate_prop(&mut self) {
        let inst = match self.selected() {
            Some(i) => i.clone(),
            None => return,
        };
        let rows = prop_rows(&inst);
        let row = match rows.get(self.psel) {
            Some(r) => r.clone_row(),
            None => return,
        };
        match row.kind {
            PKind::Name => self.prompt("Rename", &inst.name, Purpose::Rename(inst.id.clone())),
            PKind::Class | PKind::Id => self.fail("This field is read-only"),
            PKind::Tags => self.prompt("Tags (comma separated)", &inst.tags.join(", "), Purpose::Tags(inst.id.clone())),
            PKind::Source => self.open_editor(&inst.id),
            PKind::Prop(name) => {
                let value = inst.properties.iter().find(|p| p.name == name).and_then(|p| editable_text(&p.value));
                match value {
                    Some(v) => self.prompt(&format!("Edit {name}"), &v, Purpose::EditProp(inst.id.clone(), name)),
                    None => self.fail("Nested value; edit Instance.data.hjson to change it"),
                }
            }
        }
    }

    fn submit_input(&mut self, purpose: Purpose, text: String) {
        match purpose {
            Purpose::Rename(id) => {
                if let Some(path) = find_path(&self.roots, &id) {
                    self.snapshot();
                    if let Some(i) = get_mut(&mut self.roots, &path) {
                        i.name = text;
                    }
                    self.rebuild();
                    self.say("Renamed");
                }
            }
            Purpose::Tags(id) => {
                if let Some(path) = find_path(&self.roots, &id) {
                    self.snapshot();
                    if let Some(i) = get_mut(&mut self.roots, &path) {
                        i.tags = text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                    }
                    self.say("Tags updated");
                }
            }
            Purpose::EditProp(id, name) => {
                let ids = all_ids(&self.roots);
                if let Some(path) = find_path(&self.roots, &id) {
                    let before = self.roots.clone();
                    let result = match get_mut(&mut self.roots, &path) {
                        Some(i) => apply_prop_edit(i, &name, &text, &ids),
                        None => Err("instance vanished".to_string()),
                    };
                    match result {
                        Ok(()) => {
                            self.undo.push(before);
                            self.dirty = true;
                            self.say(format!("{name} updated"));
                        }
                        Err(e) => self.fail(format!("{name}: {e}")),
                    }
                }
            }
            Purpose::Insert(parent) => self.insert(parent, &text),
            Purpose::Add(id) => self.add_below(id, &text),
            Purpose::Search => {
                self.search = text;
                self.search_next();
            }
        }
    }

    // ------------------------------------------------------------ input

    fn on_key(&mut self, k: KeyEvent) -> bool {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => return self.key_normal(k),
            Mode::Menu(m) => self.key_menu(m, k),
            Mode::Input(i) => self.key_input(i, k),
            Mode::Confirm(msg, kind) => return self.key_confirm(msg, kind, k),
            Mode::Editor(ed, id) => self.key_editor(ed, id, k),
        }
        false
    }

    fn key_normal(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match k.code {
                KeyCode::Char('p') => self.paste(),
                KeyCode::Char('s') => self.save(),
                KeyCode::Char('z') => self.undo(),
                KeyCode::Char('c') | KeyCode::Char('q') => return self.try_quit(),
                _ => {}
            }
            return false;
        }
        match self.focus {
            Focus::Explorer => self.key_explorer(k),
            Focus::Props => {
                self.key_props(k);
                false
            }
        }
    }

    fn try_quit(&mut self) -> bool {
        if self.dirty {
            self.mode = Mode::Confirm("You have unsaved changes. Save before quitting?  (y = save, n = discard, Esc = cancel)".into(), ConfirmKind::Quit);
            false
        } else {
            true
        }
    }

    fn key_explorer(&mut self, k: KeyEvent) -> bool {
        let n = self.rows.len();
        match k.code {
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => {
                if self.sel + 1 < n {
                    self.sel += 1;
                }
            }
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = n.saturating_sub(1),
            KeyCode::Right => {
                if let Some(r) = self.rows.get(self.sel) {
                    if r.has_children {
                        if !r.open {
                            let id = r.id.clone();
                            self.open.insert(id);
                            self.rebuild();
                        } else if self.sel + 1 < n {
                            self.sel += 1;
                        }
                    }
                }
            }
            KeyCode::Left => {
                if let Some(r) = self.rows.get(self.sel) {
                    if r.has_children && r.open {
                        let id = r.id.clone();
                        self.open.remove(&id);
                        self.rebuild();
                    } else if r.path.len() > 1 {
                        let parent: Vec<usize> = r.path[..r.path.len() - 1].to_vec();
                        if let Some(p) = self.rows.iter().position(|x| x.path == parent) {
                            self.sel = p;
                        }
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.open_menu(),
            KeyCode::Tab => self.focus = Focus::Props,
            KeyCode::Delete | KeyCode::Char('d') => self.do_action(Action::Delete),
            KeyCode::F(2) | KeyCode::Char('r') => self.do_action(Action::Rename),
            KeyCode::Char('c') => self.do_action(Action::Copy),
            KeyCode::Char('x') => self.do_action(Action::Cut),
            KeyCode::Char('e') => self.do_action(Action::Edit),
            KeyCode::Char('a') => self.do_action(Action::Add),
            KeyCode::Char('i') | KeyCode::Insert => self.do_action(Action::Insert),
            KeyCode::Char('/') => self.prompt("Find instance by name", &self.search.clone(), Purpose::Search),
            KeyCode::Char('n') => self.search_next(),
            KeyCode::Char('*') => {
                // expand everything
                let mut ids = HashSet::new();
                for r in &self.roots {
                    collect_ids(r, &mut ids);
                }
                self.open = ids;
                self.rebuild();
            }
            KeyCode::Char('-') => {
                self.open.clear();
                self.rebuild();
                self.sel = 0;
            }
            KeyCode::Char('q') | KeyCode::Esc => return self.try_quit(),
            _ => {}
        }
        self.psel = 0;
        false
    }

    fn key_props(&mut self, k: KeyEvent) {
        let count = self.selected().map(|i| prop_rows(i).len()).unwrap_or(0);
        match k.code {
            KeyCode::Up => self.psel = self.psel.saturating_sub(1),
            KeyCode::Down => {
                if self.psel + 1 < count {
                    self.psel += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_prop(),
            KeyCode::Tab | KeyCode::Esc => self.focus = Focus::Explorer,
            _ => {}
        }
    }

    fn key_menu(&mut self, mut m: Menu, k: KeyEvent) {
        match k.code {
            KeyCode::Up => m.sel = m.sel.saturating_sub(1),
            KeyCode::Down => {
                if m.sel + 1 < m.items.len() {
                    m.sel += 1;
                }
            }
            KeyCode::Esc => return,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let a = m.items[m.sel].0;
                self.do_action(a);
                return;
            }
            _ => {}
        }
        self.mode = Mode::Menu(m);
    }

    fn key_input(&mut self, mut i: Input, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                let text: String = i.buf.iter().collect();
                self.submit_input(i.purpose, text);
                return;
            }
            KeyCode::Left => i.cur = i.cur.saturating_sub(1),
            KeyCode::Right => i.cur = (i.cur + 1).min(i.buf.len()),
            KeyCode::Home => i.cur = 0,
            KeyCode::End => i.cur = i.buf.len(),
            KeyCode::Backspace => {
                if i.cur > 0 {
                    i.buf.remove(i.cur - 1);
                    i.cur -= 1;
                }
            }
            KeyCode::Delete => {
                if i.cur < i.buf.len() {
                    i.buf.remove(i.cur);
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                i.buf.insert(i.cur, c);
                i.cur += 1;
            }
            _ => {}
        }
        self.mode = Mode::Input(i);
    }

    fn key_confirm(&mut self, msg: String, kind: ConfirmKind, k: KeyEvent) -> bool {
        let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Enter);
        let no = matches!(k.code, KeyCode::Char('n') | KeyCode::Esc);
        match kind {
            ConfirmKind::Delete(id) => {
                if yes {
                    self.delete(&id);
                } else if !no {
                    self.mode = Mode::Confirm(msg, ConfirmKind::Delete(id));
                }
                false
            }
            ConfirmKind::Quit => {
                if yes {
                    self.save();
                    return !self.dirty;
                }
                if k.code == KeyCode::Char('n') {
                    return true;
                }
                if !no {
                    self.mode = Mode::Confirm(msg, ConfirmKind::Quit);
                }
                false
            }
        }
    }

    fn key_editor(&mut self, mut ed: Box<Editor>, id: String, k: KeyEvent) {
        match ed.on_key(k) {
            EditorEvent::Exit => {
                self.apply_editor(&ed, &id);
                self.say("Script updated (Ctrl+S writes the project to disk)");
                return;
            }
            EditorEvent::Save => {
                self.apply_editor(&ed, &id);
                ed.mark_saved();
            }
            EditorEvent::None => {}
        }
        self.mode = Mode::Editor(ed, id);
    }

    fn apply_editor(&mut self, ed: &Editor, id: &str) {
        let text = ed.text();
        if let Some(path) = find_path(&self.roots, id) {
            let changed = get(&self.roots, &path).map(|i| i.source.as_deref() != Some(text.as_str())).unwrap_or(false);
            if changed {
                self.snapshot();
                if let Some(i) = get_mut(&mut self.roots, &path) {
                    i.source = Some(text);
                }
            }
        }
    }

    fn on_paste(&mut self, s: String) {
        match &mut self.mode {
            Mode::Editor(ed, _) => ed.on_paste(&s),
            Mode::Input(i) => {
                for c in s.chars().filter(|c| *c != '\n' && *c != '\r') {
                    i.buf.insert(i.cur, c);
                    i.cur += 1;
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, w: usize, h: usize) {
        if let Mode::Editor(ed, _) = &mut self.mode {
            ed.on_mouse(m);
            return;
        }
        let (x, y) = (m.column as usize, m.row as usize);
        let (ex_h, pr_h) = layout(h);
        if let Mode::Menu(menu) = &self.mode {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                let (bx, by, bw, bh) = menu_geom(w, h, &menu.items);
                if x > bx && x < bx + bw - 1 && y > by && y < by + bh - 1 {
                    let idx = y - by - 1;
                    if idx < menu.items.len() {
                        let a = menu.items[idx].0;
                        self.mode = Mode::Normal;
                        self.do_action(a);
                        return;
                    }
                }
                self.mode = Mode::Normal;
            }
            return;
        }
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let d: isize = if matches!(m.kind, MouseEventKind::ScrollUp) { -3 } else { 3 };
                if y <= ex_h {
                    let n = self.rows.len() as isize;
                    self.sel = (self.sel as isize + d).clamp(0, (n - 1).max(0)) as usize;
                } else {
                    let count = self.selected().map(|i| prop_rows(i).len()).unwrap_or(0) as isize;
                    self.psel = (self.psel as isize + d).clamp(0, (count - 1).max(0)) as usize;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if y >= 1 && y <= ex_h {
                    let idx = self.scroll + y - 1;
                    if idx < self.rows.len() {
                        let was_sel = self.sel == idx && self.focus == Focus::Explorer;
                        self.focus = Focus::Explorer;
                        self.sel = idx;
                        let r = &self.rows[idx];
                        let arrow_x = r.depth * 2 + 1;
                        if r.has_children && x >= arrow_x.saturating_sub(1) && x <= arrow_x + 1 {
                            let id = r.id.clone();
                            if !self.open.remove(&id) {
                                self.open.insert(id);
                            }
                            self.rebuild();
                        } else if was_sel {
                            self.open_menu();
                        }
                        self.psel = 0;
                    }
                } else if y >= ex_h + 2 && y < ex_h + 2 + pr_h {
                    let idx = self.pscroll + (y - (ex_h + 2));
                    let count = self.selected().map(|i| prop_rows(i).len()).unwrap_or(0);
                    if idx < count {
                        let was = self.psel == idx && self.focus == Focus::Props;
                        self.focus = Focus::Props;
                        self.psel = idx;
                        if was {
                            self.activate_prop();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    fn ensure_visible(&mut self, h: usize) {
        let (ex_h, pr_h) = layout(h);
        if ex_h > 0 {
            if self.sel < self.scroll {
                self.scroll = self.sel;
            }
            if self.sel >= self.scroll + ex_h {
                self.scroll = self.sel + 1 - ex_h;
            }
        }
        if pr_h > 0 {
            if self.psel < self.pscroll {
                self.pscroll = self.psel;
            }
            if self.psel >= self.pscroll + pr_h {
                self.pscroll = self.psel + 1 - pr_h;
            }
        }
    }

    fn draw(&mut self, scr: &mut Screen) {
        if let Mode::Editor(ed, _) = &mut self.mode {
            ed.draw(scr);
            return;
        }
        let (w, h) = (scr.w, scr.h);
        if w < 40 || h < 12 {
            scr.text(0, 0, "Terminal too small (need 40x12)", tui::NORMAL, w);
            return;
        }
        self.ensure_visible(h);
        let (ex_h, pr_h) = layout(h);

        // title bar
        scr.fill(0, 0, w, 1, tui::BAR);
        let title = format!(
            " Stewdio Explorer - {}{}",
            self.project.file.project.name,
            if self.dirty { "  [unsaved]" } else { "" }
        );
        scr.text(0, 0, &title, tui::bold(tui::BAR), w);

        // explorer
        for line in 0..ex_h {
            let idx = self.scroll + line;
            let y = 1 + line;
            let r = match self.rows.get(idx) {
                Some(r) => r,
                None => break,
            };
            let selected = idx == self.sel;
            let base = if selected {
                if self.focus == Focus::Explorer { tui::SEL } else { tui::SEL_DIM }
            } else {
                tui::NORMAL
            };
            if selected {
                scr.fill(0, y, w, 1, base);
            }
            let x0 = r.depth * 2;
            let arrow = if r.has_children { if r.open { '▾' } else { '▸' } } else { ' ' };
            scr.put(x0 + 1, y, arrow, if selected { base } else { st(Color::Yellow, Color::Reset) });
            let name_style = if selected { base } else { class_style(&r.class) };
            let used = scr.text(x0 + 3, y, &r.name, name_style, w.saturating_sub(x0 + 3));
            let class_txt = format!("  {}", r.class);
            let cs = if selected { base } else { tui::DIM };
            scr.text(x0 + 3 + used, y, &class_txt, cs, w.saturating_sub(x0 + 3 + used));
        }
        if self.rows.is_empty() {
            scr.text(2, 2, "This project has no instances. Press Enter to insert one.", tui::DIM, w);
        }

        // divider / properties tab
        let dy = ex_h + 1;
        scr.fill(0, dy, w, 1, if self.focus == Focus::Props { tui::BAR } else { st(Color::Black, Color::DarkGrey) });
        let sel_name = self.selected().map(|i| format!("{} ({})", i.name, i.class)).unwrap_or_else(|| "nothing selected".into());
        scr.text(
            0,
            dy,
            &format!(" Properties - {sel_name}"),
            tui::bold(if self.focus == Focus::Props { tui::BAR } else { st(Color::Black, Color::DarkGrey) }),
            w,
        );
        if let Some(inst) = self.selected() {
            let rows = prop_rows(inst);
            let key_w = rows.iter().map(|r| r.key.chars().count()).max().unwrap_or(8).clamp(8, 28);
            for line in 0..pr_h {
                let idx = self.pscroll + line;
                let y = dy + 1 + line;
                let r = match rows.get(idx) {
                    Some(r) => r,
                    None => break,
                };
                let selected = self.focus == Focus::Props && idx == self.psel;
                let (ks, vs) = if selected {
                    (tui::SEL, tui::SEL)
                } else {
                    (st(Color::Cyan, Color::Reset), tui::NORMAL)
                };
                if selected {
                    scr.fill(0, y, w, 1, tui::SEL);
                }
                scr.text(1, y, &r.key, ks, key_w);
                scr.text(key_w + 3, y, &r.val, vs, w.saturating_sub(key_w + 4));
            }
        }

        // status line
        let sy = h - 1;
        let sstyle = if self.status_err { tui::ERR } else { tui::BAR };
        scr.fill(0, sy, w, 1, sstyle);
        let text = if !self.status.is_empty() {
            format!(" {}", self.status)
        } else if let Some(s) = &self.stale {
            format!(" ! {s}")
        } else {
            " Arrows move  Space/Enter menu  a add  i insert child  Tab properties  Ctrl+P paste  Ctrl+S save  Ctrl+Z undo  / find  q quit".to_string()
        };
        scr.text(0, sy, &text, sstyle, w);

        // overlays
        match &self.mode {
            Mode::Menu(m) => {
                let (bx, by, bw, bh) = menu_geom(w, h, &m.items);
                let (ix, iy, iw, _) = scr.boxed(bx, by, bw, bh, "Actions", tui::BOX);
                for (i, (_, label)) in m.items.iter().enumerate() {
                    let s = if i == m.sel { tui::BOX_SEL } else { tui::BOX };
                    scr.fill(ix, iy + i, iw, 1, s);
                    scr.text(ix + 1, iy + i, label, s, iw.saturating_sub(1));
                }
            }
            Mode::Input(i) => {
                let bw = (w - 4).min(64);
                let bx = (w - bw) / 2;
                let by = h / 2 - 2;
                let (ix, iy, iw, _) = scr.boxed(bx, by, bw, 5, &i.title, tui::BOX);
                scr.text(ix, iy + 2, "Enter = OK   Esc = cancel", st(Color::Grey, Color::DarkBlue), iw);
                let shown_from = i.cur.saturating_sub(iw.saturating_sub(2));
                let text: String = i.buf.iter().skip(shown_from).collect();
                scr.text(ix + 1, iy, &text, tui::BOX, iw.saturating_sub(2));
                let cx = ix + 1 + (i.cur - shown_from);
                let under = i.buf.get(i.cur).copied().unwrap_or(' ');
                scr.put(cx, iy, under, st(Color::Black, Color::White));
            }
            Mode::Confirm(msg, _) => {
                let bw = (w - 4).min(70);
                let bx = (w - bw) / 2;
                let by = h / 2 - 2;
                let (ix, iy, iw, _) = scr.boxed(bx, by, bw, 5, "Confirm", tui::ERR);
                scr.text(ix + 1, iy, msg, tui::ERR, iw.saturating_sub(2));
                scr.text(ix + 1, iy + 2, "y = yes   n / Esc = no", tui::ERR, iw.saturating_sub(2));
            }
            _ => {}
        }
    }
}

impl PRow {
    fn clone_row(&self) -> PRow {
        PRow { key: self.key.clone(), val: self.val.clone(), kind: self.kind.clone() }
    }
}

pub fn run(_a: ExplorerArgs) -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root, project.is_model());
    let errors = loaded.diags.iter().filter(|d| d.is_error).count();
    if errors > 0 {
        for d in loaded.diags.iter().filter(|d| d.is_error) {
            ui::error(&d.msg);
        }
        return Err(CliError::new(
            PROJECT,
            "Fix the errors above first (a partial project could be overwritten when you save)",
        ));
    }
    let database = Api::load();
    let mut app = App::new(project, loaded.roots, database);
    let term = Term::enter().map_err(|e| CliError::new(GENERAL, format!("cannot start the terminal UI: {e}")))?;
    let result = event_loop(&mut app);
    drop(term);
    result.map_err(|e| CliError::new(GENERAL, format!("terminal error: {e}")))?;
    Ok(())
}

fn event_loop(app: &mut App) -> io::Result<()> {
    let mut out = io::stdout();
    loop {
        let (w, h) = terminal::size()?;
        let mut scr = Screen::new(w as usize, h as usize);
        app.draw(&mut scr);
        scr.flush(&mut out)?;
        match event::read()? {
            Event::Key(k) => {
                if k.kind == KeyEventKind::Release {
                    continue;
                }
                app.status.clear();
                app.status_err = false;
                if app.on_key(k) {
                    return Ok(());
                }
            }
            Event::Mouse(m) => app.on_mouse(m, w as usize, h as usize),
            Event::Paste(s) => app.on_paste(s),
            _ => {}
        }
    }
}
