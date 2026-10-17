//! Analyze › Notebook (New Notebook, Add and Open Notebook): notebooks of text and command cells.
//! A command cell is a script (one command per line, `id {json}`) whose results show beneath it.
//! Notebooks save as RustGIS notebooks or Jupyter notebooks, and open from either.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

/// Notebook files: JSON with this extension (`.ipynb` is read and written too).
pub const EXTENSION: &str = "rgisnb";
/// Most cells a notebook keeps, and the longest cell and output kept.
const MAX_CELLS: usize = 1000;
const MAX_SOURCE: usize = 200_000;
const MAX_OUTPUT: usize = 20_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notebook {
    pub name: String,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub kind: CellKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default)]
    pub error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellKind {
    /// Text (Markdown).
    Markdown,
    /// Commands, run in order.
    Code,
}

impl Cell {
    fn new(kind: CellKind, source: &str) -> Cell {
        Cell { kind, source: source.chars().take(MAX_SOURCE).collect(), output: None, error: false }
    }
}

impl Notebook {
    pub(crate) fn clamp(&mut self) {
        self.name = self.name.chars().take(200).collect();
        self.cells.truncate(MAX_CELLS);
        for c in &mut self.cells {
            c.source = c.source.chars().take(MAX_SOURCE).collect();
            c.output = c.output.as_ref().map(|o| o.chars().take(MAX_OUTPUT).collect());
        }
    }
}

fn need_notebook(s: &Session) -> Option<&'static str> {
    if s.project.notebooks.is_empty() { Some("add a notebook first (Analyze › New Notebook)") } else { None }
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("notebook.new", "New Notebook", "Analyze › Notebook", |s, p| {
            let name = unique_name(s, ps(p, "name").unwrap_or("Notebook"));
            let nb = Notebook {
                name: name.clone(),
                cells: vec![
                    Cell::new(CellKind::Markdown, &format!("# {name}\n\nCommand cells run one command per line: `id {{json}}`.")),
                    Cell::new(CellKind::Code, "# For example:\n# map.goToXY {\"lon\": 12.5, \"lat\": 41.9, \"scale\": 2000000}"),
                ],
            };
            let r = push(s, nb)?;
            // Trusted only once it exists.
            s.trusted_notebooks.insert(name.to_ascii_lowercase());
            Ok(r)
        })
        .params(r#"{"name"?}"#),
        C::new("notebook.open", "Add and Open Notebook", "Analyze › Notebook", |s, p| {
            let text = super::items::read_text(p)?;
            let fallback =
                ps(p, "path").map(|x| Path::new(x).file_stem().map_or_else(|| x.to_string(), |n| n.to_string_lossy().into_owned()));
            let mut nb = parse(&text, fallback.as_deref().unwrap_or("Notebook"))?;
            nb.clamp();
            nb.name = unique_name(s, &nb.name);
            // Trust left over from a removed or undone notebook of this name doesn't carry over.
            s.trusted_notebooks.remove(&nb.name.to_ascii_lowercase());
            push(s, nb)
        })
        .params(r#"{"path"?: "*.rgisnb|*.ipynb", "text"?: notebook JSON, "data"?: base64 file}"#),
        C::new("notebook.activate", "Open Notebook", "Browser", |s, p| {
            let i = index(s, p)?;
            s.active_notebook = Some(i);
            Ok(json!({"notebook": i}))
        })
        .view()
        .when(need_notebook)
        .params(r#"{"notebook": name|index}"#),
        C::new("notebook.remove", "Delete Notebook", "Browser", |s, p| {
            let i = index(s, p)?;
            let nb = s.project.notebooks.remove(i);
            s.trusted_notebooks.remove(&nb.name.to_ascii_lowercase());
            s.active_notebook = None;
            Ok(Value::Null)
        })
        .when(need_notebook)
        .params(r#"{"notebook"?}"#),
        C::new("notebook.addCell", "Add Cell", "Notebook", |s, p| {
            let i = index(s, p)?;
            let kind = match ps(p, "kind").unwrap_or("code") {
                "code" => CellKind::Code,
                "markdown" | "text" => CellKind::Markdown,
                k => return Err(format!("unknown cell kind '{k}'")),
            };
            let nb = s.project.notebooks.get_mut(i).ok_or("no notebook")?;
            if nb.cells.len() >= MAX_CELLS {
                return Err(format!("notebooks hold at most {MAX_CELLS} cells"));
            }
            let at =
                p.get("at").and_then(Value::as_u64).and_then(|k| usize::try_from(k).ok()).unwrap_or(nb.cells.len()).min(nb.cells.len());
            nb.cells.insert(at, Cell::new(kind, ps(p, "source").unwrap_or("")));
            Ok(json!({"cell": at, "cells": nb.cells.len()}))
        })
        .when(need_notebook)
        .params(r#"{"notebook"?, "kind"?: "code"|"markdown", "source"?, "at"?: index}"#),
        C::new("notebook.editCell", "Edit Cell", "Notebook", |s, p| {
            let (i, k) = cell_of(s, p)?;
            let source = ps(p, "source").ok_or("source is required")?;
            let c = s.project.notebooks.get_mut(i).and_then(|nb| nb.cells.get_mut(k)).ok_or("no cell")?;
            c.source = source.chars().take(MAX_SOURCE).collect();
            Ok(json!({"cell": k}))
        })
        .when(need_notebook)
        .params(r#"{"notebook"?, "cell": index, "source": text}"#),
        C::new("notebook.removeCell", "Delete Cell", "Notebook", |s, p| {
            let (i, k) = cell_of(s, p)?;
            let nb = s.project.notebooks.get_mut(i).ok_or("no notebook")?;
            nb.cells.remove(k);
            Ok(json!({"cells": nb.cells.len()}))
        })
        .when(need_notebook)
        .params(r#"{"notebook"?, "cell": index}"#),
        C::new("notebook.run", "Run Cells", "Notebook", run)
            .when(need_notebook)
            .params(r#"{"notebook"?, "cell"?: index (default: every command cell, stopping at an error)}"#),
        C::new("notebook.save", "Save Notebook", "Notebook", |s, p| {
            let path = ps(p, "path").ok_or("path is required")?;
            let i = index(s, p)?;
            let bytes = notebook_file(s, i, path.to_ascii_lowercase().ends_with(".ipynb"))?;
            rustgis_data::io::write_atomic(Path::new(path), &bytes).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({"path": path}))
        })
        .view()
        .when(need_notebook)
        .params(r#"{"notebook"?, "path": "*.rgisnb|*.ipynb"}"#),
    ]
}

fn push(s: &mut Session, nb: Notebook) -> CmdResult {
    let name = nb.name.clone();
    if s.project.notebooks.len() >= crate::project::MAX_ITEMS {
        return Err(format!("a project holds at most {} notebooks", crate::project::MAX_ITEMS));
    }
    s.project.notebooks.push(nb);
    let i = s.project.notebooks.len() - 1;
    s.active_notebook = Some(i);
    Ok(json!({"notebook": i, "name": name}))
}

/// The notebook named or numbered by `notebook`, else the active one, else the last.
pub fn index(s: &Session, p: &Value) -> Result<usize, String> {
    let nbs = &s.project.notebooks;
    match p.get("notebook") {
        Some(Value::Number(n)) => {
            n.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|i| *i < nbs.len()).ok_or_else(|| format!("no notebook {n}"))
        }
        Some(Value::String(n)) => nbs
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(n))
            .or_else(|| n.parse::<usize>().ok().filter(|i| *i < nbs.len()))
            .ok_or_else(|| format!("no notebook named '{n}'")),
        _ => s
            .active_notebook
            .filter(|i| *i < nbs.len())
            .or(nbs.len().checked_sub(1))
            .ok_or_else(|| "no notebooks yet: add one with notebook.new".to_string()),
    }
}

fn cell_of(s: &Session, p: &Value) -> Result<(usize, usize), String> {
    let i = index(s, p)?;
    let n = s.project.notebooks.get(i).map_or(0, |nb| nb.cells.len());
    let k = p.get("cell").and_then(Value::as_u64).and_then(|k| usize::try_from(k).ok()).ok_or("cell is required")?;
    if k >= n {
        return Err(format!("no cell {k}"));
    }
    Ok((i, k))
}

fn unique_name(s: &Session, base: &str) -> String {
    let base: String = base.trim().chars().take(200).collect();
    let base = if base.is_empty() { "Notebook".to_string() } else { base };
    let taken = |n: &str| s.project.notebooks.iter().any(|r| r.name.eq_ignore_ascii_case(n));
    let mut name = base.clone();
    let mut k = 1;
    while taken(&name) {
        k += 1;
        name = format!("{base} {k}");
    }
    name
}

fn run(s: &mut Session, p: &Value) -> CmdResult {
    if s.notebook_running {
        return Err("a notebook is already running (a cell can't run notebooks, even through a task or model)".into());
    }
    s.notebook_running = true;
    // The flag is cleared on every path, a panic included.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_cells(s, p)))
        .unwrap_or_else(|_| Err("the notebook failed unexpectedly".into()));
    s.notebook_running = false;
    r
}

fn run_cells(s: &mut Session, p: &Value) -> CmdResult {
    let i = index(s, p)?;
    let cells: Vec<usize> = match p.get("cell") {
        Some(_) => vec![cell_of(s, p)?.1],
        None => s.project.notebooks.get(i).map(|nb| (0..nb.cells.len()).collect()).unwrap_or_default(),
    };
    // Commands in a cell may add or remove notebooks, so this one is found by name each time.
    let name = s.project.notebooks.get(i).map(|nb| nb.name.clone()).unwrap_or_default();
    let find = |s: &Session| s.project.notebooks.iter().position(|nb| nb.name.eq_ignore_ascii_case(&name));
    let mut ran = 0;
    for k in cells {
        let Some(i) = find(s) else { break };
        let Some(c) = s.project.notebooks.get(i).and_then(|nb| nb.cells.get(k)) else { break };
        if c.kind != CellKind::Code {
            continue;
        }
        let script = c.source.clone();
        // A notebook from a file (opened, or in an opened project) is someone else's: its
        // cells may not read or write files or run other scripts.
        let trusted = s.project.notebooks.get(i).is_some_and(|nb| s.trusted_notebooks.contains(&nb.name.to_ascii_lowercase()));
        let r = match (trusted, untrusted_line(&script)) {
            (false, Some(n)) => Err(format!(
                "line {n} reads or writes files or runs a script, which a notebook from a file can't do; copy it into a new notebook to run it"
            )),
            _ => super::workflows::run_script(s, &json!({"script": script})),
        };
        let failed = r.is_err();
        let text = match r {
            Ok(v) => {
                let lines: Vec<String> = v["ran"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| {
                        let res = &x["result"];
                        let shown = if res.is_null() { "done".to_string() } else { serde_json::to_string(res).unwrap_or_default() };
                        format!("{}: {shown}", x["command"].as_str().unwrap_or(""))
                    })
                    .collect();
                if lines.is_empty() { "(nothing to run)".to_string() } else { lines.join("\n") }
            }
            Err(e) => e,
        };
        if let Some(c) = find(s).and_then(|i| s.project.notebooks.get_mut(i)).and_then(|nb| nb.cells.get_mut(k)) {
            c.output = Some(text.chars().take(MAX_OUTPUT).collect());
            c.error = failed;
        }
        ran += 1;
        if failed {
            return Ok(json!({"ran": ran, "error": k}));
        }
    }
    Ok(json!({"ran": ran}))
}

/// The first line (1-based) of a cell whose command could reach outside the session.
fn untrusted_line(script: &str) -> Option<usize> {
    script.lines().enumerate().find_map(|(n, line)| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        // Malformed params read as none here; `run_script` rejects that line before it runs.
        let params: Value = serde_json::from_str(rest.trim()).unwrap_or(Value::Null);
        super::items::touches_files(id.trim_end_matches('='), &params).then_some(n + 1)
    })
}

/// Text of a Jupyter field: a string or a list of lines.
fn jupyter_text(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// A RustGIS or Jupyter notebook.
fn parse(text: &str, fallback_name: &str) -> Result<Notebook, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a notebook: {e}"))?;
    if v.get("nbformat").is_some() {
        let mut nb = Notebook { name: v["metadata"]["title"].as_str().unwrap_or(fallback_name).to_string(), cells: Vec::new() };
        for c in v["cells"].as_array().into_iter().flatten().take(MAX_CELLS) {
            let kind = if c["cell_type"].as_str() == Some("code") { CellKind::Code } else { CellKind::Markdown };
            let mut cell = Cell::new(kind, &jupyter_text(&c["source"]));
            let outputs = c["outputs"].as_array().into_iter().flatten();
            let out: String = outputs.clone().map(|o| jupyter_text(&o["text"])).collect();
            if !out.is_empty() {
                cell.output = Some(out.chars().take(MAX_OUTPUT).collect());
                cell.error = outputs.clone().any(|o| o["name"] == "stderr" || o["output_type"] == "error");
            }
            nb.cells.push(cell);
        }
        return Ok(nb);
    }
    let mut nb: Notebook = serde_json::from_value(v).map_err(|e| format!("not a RustGIS notebook: {e}"))?;
    if nb.name.trim().is_empty() {
        nb.name = fallback_name.to_string();
    }
    Ok(nb)
}

/// The notebook as a RustGIS notebook or (nbformat 4) Jupyter notebook.
pub fn notebook_file(s: &Session, i: usize, jupyter: bool) -> Result<Vec<u8>, String> {
    let nb = s.project.notebooks.get(i).ok_or("no notebook")?;
    let v = if jupyter {
        let lines = |t: &str| -> Value { json!(t.split_inclusive('\n').collect::<Vec<_>>()) };
        let cells: Vec<Value> = nb
            .cells
            .iter()
            .map(|c| match c.kind {
                CellKind::Markdown => json!({"cell_type": "markdown", "metadata": {}, "source": lines(&c.source)}),
                CellKind::Code => {
                    let outputs: Vec<Value> = c
                        .output
                        .iter()
                        .map(|o| json!({"output_type": "stream", "name": if c.error { "stderr" } else { "stdout" }, "text": lines(o)}))
                        .collect();
                    json!({"cell_type": "code", "metadata": {}, "execution_count": null, "outputs": outputs, "source": lines(&c.source)})
                }
            })
            .collect();
        json!({"nbformat": 4, "nbformat_minor": 5, "metadata": {"title": nb.name, "language_info": {"name": "rustgis"}}, "cells": cells})
    } else {
        serde_json::to_value(nb).map_err(|e| e.to_string())?
    };
    serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())
}
