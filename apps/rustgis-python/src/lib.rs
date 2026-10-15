//! Python bindings to the RustGIS engine: the native half of the `rustgis` package on PyPI.
//!
//! The surface is deliberately small. Everything RustGIS does is a command with an id and JSON
//! parameters, so the module exposes `run(id, params_json) -> result_json` and leaves the Pythonic
//! API (keyword arguments, `geopandas`, Jupyter display) to `python/rustgis/__init__.py`. The few
//! things that aren't commands are here too: the map as PNG bytes, a layer as GeoJSON, and the
//! notices commands leave behind.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use rustgis_engine::{Project, Session};
use serde_json::Value;

create_exception!(_rustgis, RustGISError, PyException, "A RustGIS command or file operation failed.");

/// One RustGIS session: a project with its maps, layers, undo history and selection.
///
/// Usable from any thread (Jupyter delivers a widget's messages on one of its own): every method
/// holds the GIL throughout, so calls take turns.
#[pyclass(module = "rustgis._rustgis")]
struct Engine {
    session: Session,
}

#[pymethods]
impl Engine {
    #[new]
    fn new() -> Self {
        Engine { session: Session::new(Project::default()) }
    }

    /// Runs command `id` with `params` (JSON text) and returns its result as JSON text.
    fn run(&mut self, id: &str, params: &str) -> PyResult<String> {
        let params: Value = if params.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(params).map_err(|e| PyValueError::new_err(format!("parameters aren't JSON: {e}")))?
        };
        let result = self.session.run(id, &params).map_err(|e| RustGISError::new_err(format!("{id}: {e}")))?;
        Ok(result.to_string())
    }

    /// The map as a PNG: `width` and `height` in pixels (the view's own size when left out) at
    /// `dpi` (96 when left out).
    #[pyo3(signature = (width=None, height=None, dpi=None))]
    fn render_png<'py>(
        &mut self,
        py: Python<'py>,
        width: Option<f64>,
        height: Option<f64>,
        dpi: Option<f64>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let png = rustgis_engine::cmd::share::map_png(&mut self.session, width, height, dpi).map_err(RustGISError::new_err)?;
        Ok(PyBytes::new(py, &png))
    }

    /// Layer `layer` (a name or an id) of the active map as GeoJSON text: its features as they are
    /// in memory (a large file read in place holds the features of the last view; run
    /// `layer.loadAll` first for all of them).
    fn layer_geojson(&self, layer: &str) -> PyResult<String> {
        let map = self.session.map();
        let found = map.layer_by_name(layer).or_else(|| layer.trim().parse::<u32>().ok().and_then(|id| map.layer(id)));
        let found = found.ok_or_else(|| RustGISError::new_err(format!("no layer named '{layer}'")))?;
        Ok(rustgis_engine::data::io::to_geojson(&found.dataset).to_string())
    }

    /// The project as `.rgis` JSON text, as `project.saveAs` writes it (what `project.openText`
    /// opens).
    fn project_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.session.project).map_err(|e| RustGISError::new_err(e.to_string()))
    }

    /// The notices commands have given since the last call (a file's warnings, a tool's
    /// summary): `(text, is_error)` pairs, oldest first.
    fn take_notices(&mut self) -> Vec<(String, bool)> {
        self.session.notices.drain(..).map(|n| (n.text, n.error)).collect()
    }
}

/// Loads the plugins installed for the desktop app, so their commands and tools run here too.
/// Returns the problems found, one message per plugin that couldn't load.
#[pyfunction]
fn load_plugins() -> Vec<String> {
    rustgis_engine::cmd::plugins::load_installed()
}

#[pymodule]
fn _rustgis(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("RustGISError", m.py().get_type::<RustGISError>())?;
    m.add_class::<Engine>()?;
    m.add_function(wrap_pyfunction!(load_plugins, m)?)?;
    Ok(())
}
