//! `stewdio tree` and `stewdio find`: look around a project without opening the explorer.
use clap::Args;

use crate::error::*;
use crate::fsproj;
use crate::model::*;
use crate::project::Project;
use crate::ui;

#[derive(Args)]
pub struct TreeArgs {
    /// Limit how deep the tree is printed
    #[arg(long)]
    pub depth: Option<usize>,
    /// Only list instances of this class (flat list of paths)
    #[arg(long)]
    pub class: Option<String>,
    /// Also print each instance's properties
    #[arg(long)]
    pub props: bool,
}

fn summarize(v: &PropValue) -> String {
    match v {
        PropValue::Text(t) => t.replace('\n', "⏎"),
        PropValue::Fields(f) => f.iter().map(|(k, x)| format!("{k}={}", summarize(x))).collect::<Vec<_>>().join(" "),
    }
}

fn print_tree(list: &[Instance], prefix: &str, depth: usize, max: Option<usize>, props: bool) {
    for (i, inst) in list.iter().enumerate() {
        let last = i + 1 == list.len();
        let branch = if last { "└── " } else { "├── " };
        let cont = if last { "    " } else { "│   " };
        let mark = if is_script_class(&inst.class) { ui::paint("32", " ✎", false) } else { String::new() };
        println!("{prefix}{branch}{}{mark} {}", inst.name, ui::paint("2", &format!("({})", inst.class), false));
        if props {
            for p in &inst.properties {
                println!("{prefix}{cont}  · {} = {}", p.name, summarize(&p.value));
            }
        }
        if max.map(|m| depth < m).unwrap_or(true) {
            print_tree(&inst.children, &format!("{prefix}{cont}"), depth + 1, max, props);
        } else if !inst.children.is_empty() {
            let n: usize = inst.children.iter().map(|c| c.count()).sum();
            println!("{prefix}{cont}└── … {n} hidden");
        }
    }
}

fn collect_class(list: &[Instance], prefix: &str, class: &str, out: &mut Vec<String>) {
    for i in list {
        let path = if prefix.is_empty() { i.name.clone() } else { format!("{prefix}/{}", i.name) };
        if i.class.eq_ignore_ascii_case(class) {
            out.push(path.clone());
        }
        collect_class(&i.children, &path, class, out);
    }
}

pub fn tree(a: TreeArgs) -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root, project.is_model());
    if let Some(class) = &a.class {
        let mut out = Vec::new();
        collect_class(&loaded.roots, "", class, &mut out);
        for p in &out {
            println!("{p}");
        }
        println!("\n{} instance(s) of class {class}", out.len());
        return Ok(());
    }
    println!("{}", ui::paint("1", &project.file.project.name, false));
    print_tree(&loaded.roots, "", 0, a.depth, a.props);
    let total: usize = loaded.roots.iter().map(|r| r.count()).sum();
    let scripts: usize = loaded.roots.iter().map(|r| r.script_count()).sum();
    println!("\n{total} instances, {scripts} scripts");
    Ok(())
}

#[derive(Args)]
pub struct FindArgs {
    /// Text to look for
    pub query: String,
    /// Only look at instances of this class
    #[arg(long)]
    pub class: Option<String>,
    /// Search instance names
    #[arg(long)]
    pub names: bool,
    /// Search script source
    #[arg(long)]
    pub scripts: bool,
    /// Search property values
    #[arg(long)]
    pub props: bool,
    /// Case-sensitive search
    #[arg(long)]
    pub case: bool,
    /// Maximum number of matching instances to print
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
}

struct Finder<'a> {
    args: &'a FindArgs,
    needle: String,
    hits: usize,
}

impl<'a> Finder<'a> {
    fn norm(&self, s: &str) -> String {
        if self.args.case {
            s.to_string()
        } else {
            s.to_lowercase()
        }
    }

    fn walk(&mut self, list: &[Instance], prefix: &str) {
        for i in list {
            if self.hits >= self.args.limit {
                return;
            }
            let path = if prefix.is_empty() { i.name.clone() } else { format!("{prefix}/{}", i.name) };
            let class_ok = self.args.class.as_ref().map(|c| i.class.eq_ignore_ascii_case(c)).unwrap_or(true);
            if class_ok {
                let all = !(self.args.names || self.args.scripts || self.args.props);
                let mut details: Vec<String> = Vec::new();
                let mut matched = false;
                if (all || self.args.names) && self.norm(&i.name).contains(&self.needle) {
                    matched = true;
                }
                if all || self.args.scripts {
                    if let Some(src) = &i.source {
                        let mut shown = 0;
                        for (n, line) in src.lines().enumerate() {
                            if self.norm(line).contains(&self.needle) {
                                matched = true;
                                if shown < 5 {
                                    details.push(format!("L{}: {}", n + 1, line.trim()));
                                }
                                shown += 1;
                            }
                        }
                        if shown > 5 {
                            details.push(format!("… {} more line(s)", shown - 5));
                        }
                    }
                }
                if all || self.args.props {
                    for p in &i.properties {
                        let v = summarize(&p.value);
                        if self.norm(&v).contains(&self.needle) || self.norm(&p.name).contains(&self.needle) {
                            matched = true;
                            details.push(format!("{} = {}", p.name, v.chars().take(80).collect::<String>()));
                        }
                    }
                }
                if matched {
                    self.hits += 1;
                    println!("{} {}", path, ui::paint("2", &format!("({})", i.class), false));
                    for d in details {
                        println!("    {d}");
                    }
                }
            }
            self.walk(&i.children, &path);
        }
    }
}

pub fn find(a: FindArgs) -> CliResult<()> {
    let project = Project::current()?;
    let loaded = fsproj::load(&project.root, project.is_model());
    let needle = if a.case { a.query.clone() } else { a.query.to_lowercase() };
    let mut f = Finder { args: &a, needle, hits: 0 };
    f.walk(&loaded.roots, "");
    let hits = f.hits;
    if hits == 0 {
        println!("No matches for '{}'", a.query);
    } else if hits >= a.limit {
        println!("\n(stopped after {hits} matches; use --limit to see more)");
    } else {
        println!("\n{hits} matching instance(s)");
    }
    Ok(())
}
