//! Project tab (backstage), undo/redo and tool switching.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Project, Session, Tool};

pub fn commands() -> Vec<C> {
    vec![
        // Replacing or saving the project isn't an undoable edit: these are `.view()` commands
        // that manage `dirty` and the history themselves.
        C::new("project.new", "New Project", "Project", new).key("Mod+N").view().params(r#"{"name"?: string}"#),
        C::new("project.sample", "World Atlas Sample", "Project", sample).view(),
        C::new("project.open", "Open Project", "Project", open).key("Mod+O").view().params(r#"{"path": string}"#),
        C::new("project.openText", "Open Project (from text)", "Project", open_text).view().params(r#"{"text": project JSON, "fileName"?: string}"#),
        C::new("project.save", "Save Project", "Project", save).key("Mod+S").view().params(r#"{"path"?: string}"#),
        C::new("project.saveAs", "Save Project As", "Project", save_as).key("Mod+Shift+S").view().params(r#"{"path": string}"#),
        C::new("project.rename", "Rename Project", "Project", rename).params(r#"{"name": string}"#),
        C::new("edit.undo", "Undo", "Quick Access Toolbar", |s, _| if s.undo() { Ok(Value::Null) } else { Err("nothing to undo".into()) })
            .key("Mod+Z")
            .view()
            .when(|s| if s.can_undo() { None } else { Some("nothing to undo") }),
        C::new("edit.redo", "Redo", "Quick Access Toolbar", |s, _| if s.redo() { Ok(Value::Null) } else { Err("nothing to redo".into()) })
            .key("Mod+Y")
            .view()
            .when(|s| if s.can_redo() { None } else { Some("nothing to redo") }),
        C::new("tool.set", "Set Map Tool", "Home › Navigate", set_tool)
            .view()
            .params(r#"{"tool": "explore"|"select"|"zoomIn"|"measure"|"measureArea"|"createPoint"|"createLine"|"createPolygon"|"graphicPoint"|"graphicText"|"graphicLine"|"graphicCurve"|"graphicFreehand"|"graphicPolygon"|"graphicLasso"|"graphicRectangle"|"graphicCircle"|"graphicEllipse"|"graphicPicture"}"#),
        C::new("info.project", "Project Info", "Project", info).view(),
        C::new("info.commands", "List Commands", "Help", list).view(),
    ]
}

/// Replaces the project, clearing history and per-project state.
pub(crate) fn reset_to(s: &mut Session, p: Project) {
    reset(s, p);
}

fn reset(s: &mut Session, p: Project) {
    s.project = p;
    s.active_layer = s.map().layers.first().map(|l| l.id);
    s.table_layer = None;
    s.table_standalone = None;
    s.active_layout = None;
    s.tool = Tool::Explore;
    s.clear_history();
    s.history_from_file = s.project.gp_history.len();
}

fn new(s: &mut Session, p: &Value) -> CmdResult {
    reset(s, Project::new(super::ps(p, "name").unwrap_or("Untitled")));
    s.path = None;
    Ok(Value::Null)
}

fn sample(s: &mut Session, _: &Value) -> CmdResult {
    reset(s, crate::sample::world_project());
    s.active_layer = s.map().layer_by_name("Countries").map(|l| l.id);
    s.path = None;
    Ok(Value::Null)
}

/// Largest project package opened (1 GiB).
const MAX_PACKAGE: u64 = 1 << 30;

fn open(s: &mut Session, p: &Value) -> CmdResult {
    let path = PathBuf::from(super::ps(p, "path").ok_or("path is required")?);
    let is_package = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("rgispkg"));
    let proj = if is_package {
        // Read at most a byte past the limit, so a huge file is refused rather than loaded.
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|f| f.take(MAX_PACKAGE + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() as u64 > MAX_PACKAGE {
            return Err("the package is too large".into());
        }
        Project::from_json(&super::items::package_text(&bytes)?)?
    } else {
        Project::load(&path)?
    };
    reset(s, proj);
    // A package opens as a new, unsaved project (saving writes a plain project file).
    if !is_package {
        name_from_file(s, &path);
    }
    s.path = if is_package { None } else { Some(path) };
    s.notify(format!("Opened {}", s.project.name), false);
    Ok(Value::Null)
}

fn open_text(s: &mut Session, p: &Value) -> CmdResult {
    let text = super::ps(p, "text").ok_or("text is required")?;
    let proj = Project::from_json(text)?;
    reset(s, proj);
    // The browser opens files by name and contents: the name still names the project.
    if let Some(file) = super::ps(p, "fileName") {
        name_from_file(s, std::path::Path::new(file));
    }
    s.path = None;
    s.notify(format!("Opened {}", s.project.name), false);
    Ok(Value::Null)
}

fn write(s: &mut Session, path: PathBuf) -> CmdResult {
    let path = if path.extension().is_none() { path.with_extension(crate::project::EXTENSION) } else { path };
    s.project.save(&path)?;
    s.dirty = false;
    s.edit_counts.clear();
    s.notify(format!("Saved {}", path.display()), false);
    s.path = Some(path.clone());
    Ok(json!({"path": path.display().to_string()}))
}

/// Names the project after its file, as Save As does, so a renamed or copied file opens under
/// its own name.
fn name_from_file(s: &mut Session, file: &std::path::Path) {
    if let Some(stem) = file.file_stem().and_then(|x| x.to_str()).map(str::trim).filter(|x| !x.is_empty()) {
        s.project.name = stem.chars().take(200).collect();
    }
}

fn save(s: &mut Session, p: &Value) -> CmdResult {
    let path = match super::ps(p, "path") {
        Some(x) => PathBuf::from(x),
        None => s.path.clone().ok_or("this project hasn't been saved yet; use Save As")?,
    };
    write(s, path)
}

fn save_as(s: &mut Session, p: &Value) -> CmdResult {
    let path = PathBuf::from(super::ps(p, "path").ok_or("path is required")?);
    if let Some(stem) = path.file_stem().and_then(|x| x.to_str()) {
        s.project.name = stem.to_string();
    }
    write(s, path)
}

fn rename(s: &mut Session, p: &Value) -> CmdResult {
    let n = super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("name is required")?;
    s.project.name = n.chars().take(200).collect();
    Ok(Value::Null)
}

fn set_tool(s: &mut Session, p: &Value) -> CmdResult {
    let t = parse_tool(p)?;
    if t.creates() {
        let id = s.layer_id(&Value::Null).map_err(|_| "select the layer to edit in the Layers pane first".to_string())?;
        let gt = s.map().layer(id).map(|l| l.geometry_type());
        let ok = matches!(
            (t, gt),
            (Tool::CreatePoint, Some(rustgis_data::GeometryType::Point))
                | (Tool::CreateLine, Some(rustgis_data::GeometryType::Polyline))
                | (Tool::CreatePolygon, Some(rustgis_data::GeometryType::Polygon))
        );
        if !ok {
            return Err(format!("{} needs a {} layer", t.label(), t.label().trim_start_matches("Create ").to_lowercase()));
        }
    }
    s.tool = t;
    Ok(Value::Null)
}

fn parse_tool(p: &Value) -> Result<Tool, String> {
    let name = ps(p, "tool").ok_or("tool is required")?;
    Tool::from_code(name).ok_or_else(|| format!("unknown tool '{name}'"))
}

fn info(s: &mut Session, _: &Value) -> CmdResult {
    let m = s.map();
    Ok(json!({
        "name": s.project.name,
        "path": s.path.as_ref().map(|p| p.display().to_string()),
        "maps": s.project.maps.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
        "activeMap": m.name,
        "basemap": m.basemap.code(),
        "projection": m.projection.code(),
        "scale": m.scale().round(),
        "center": {"lon": m.center_lonlat().x, "lat": m.center_lonlat().y},
        "layers": m.layers.iter().map(|l| match &l.raster {
            Some(rl) => json!({
                "id": l.id, "name": l.name, "type": "Raster", "visible": l.visible, "renderer": rl.style.renderer.label(),
                "size": rl.data.as_ref().map(|d| json!([d.width, d.height, d.band_count()])),
                "missing": rl.data.is_none(),
            }),
            None => json!({
                "id": l.id, "name": l.name, "type": l.geometry_type().label(), "features": l.dataset.features.len(),
                "visible": l.visible, "renderer": l.renderer.kind_label(), "selected": l.selection.len(),
                "fields": l.dataset.fields.iter().map(|f| format!("{} ({})", f.name, f.ty.label())).collect::<Vec<_>>(),
            }),
        }).collect::<Vec<_>>(),
        "tables": s.project.tables.iter().map(|t| json!({
            "id": t.id, "name": t.name, "rows": t.features.len(),
            "fields": t.fields.iter().map(|f| format!("{} ({})", f.name, f.ty.label())).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let builtin = s
        .registry
        .commands
        .iter()
        .map(|c| json!({"id": c.id, "label": c.label, "location": c.location, "shortcut": c.shortcut, "params": c.params}));
    let plugins = rustgis_plugins::commands().into_iter().map(|c| {
        json!({"id": c.id, "label": c.decl.label, "location": format!("Plugins › {}", c.plugin.manifest.name), "shortcut": "", "params": c.decl.params})
    });
    Ok(Value::Array(builtin.chain(plugins).collect()))
}
