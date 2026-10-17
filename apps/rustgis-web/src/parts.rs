//! Large GeoTIFFs picked or dropped in the browser, read a part at a time: the header first, then
//! only the parts the overview RustGIS opens needs, by `Blob.slice`, never the whole file. The parts
//! read go to the app as one small file (`Sparse::container`), which the GeoTIFF reader opens and a
//! saved project keeps. A cloud-optimised file (tiled, with overviews) needs little of itself; one
//! in strips without overviews needs nearly all, and is refused past the upload limit.

use std::sync::{Arc, Mutex};

use rustgis_ui_egui::rustgis_engine::cmd::raster::MAX_UPLOAD_BYTES;
use rustgis_ui_egui::rustgis_engine::data::raster::cog::Sparse;
use rustgis_ui_egui::rustgis_engine::data::raster::geotiff::{self, Partial};
use rustgis_ui_egui::{i18n, tl};
use wasm_bindgen::JsCast;

/// Notices for the app from work done off the frame: the text, and whether it is an error.
pub type Notes = Arc<Mutex<Vec<(String, bool)>>>;

/// The first read: enough for a cloud-optimised file's header, as GDAL writes them.
const HEAD: u64 = 1 << 20;
/// Each later read is at least this large (and grows with what has been read, so a file needing
/// most of itself takes few rounds).
const CHUNK: u64 = 4 << 20;
/// The most reads before giving up.
const MAX_ROUNDS: usize = 256;

/// Whether a picked or dropped file is read a part at a time: a GeoTIFF over the upload limit.
pub fn wanted(name: &str, size: f64) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".tif") || lower.ends_with(".tiff")) && size > MAX_UPLOAD_BYTES as f64
}

/// Reads a picked or dropped file: a large GeoTIFF a part at a time, anything else whole. Says
/// what is happening, or what went wrong, in `notes`; `None` when the file couldn't be read.
pub async fn read_file(file: web_sys::File, notes: Notes) -> Option<Vec<u8>> {
    let name = file.name();
    let note = |text: String, error: bool| notes.lock().unwrap_or_else(|e| e.into_inner()).push((text, error));
    if wanted(&name, file.size()) {
        let mb = (MAX_UPLOAD_BYTES >> 20).to_string();
        note(i18n::fmt("Reading {name} a part at a time (it is larger than {mb} MB)…", &[("name", &name), ("mb", &mb)]), false);
        return match read_parts(&file, &name).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                note(format!("{name}: {e}"), true);
                None
            }
        };
    }
    match slice(&file, 0, file.size() as u64).await {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            note(format!("{name}: {e}"), true);
            None
        }
    }
}

/// Reads the parts of a GeoTIFF the raster RustGIS opens from it needs; returns them as a
/// container.
async fn read_parts(file: &web_sys::File, name: &str) -> Result<Vec<u8>, String> {
    let len = file.size() as u64;
    let limit = MAX_UPLOAD_BYTES as u64;
    let mut parts = Sparse::default().with_len(len).with(0, slice(file, 0, HEAD.min(len)).await?);
    for _ in 0..MAX_ROUNDS {
        match geotiff::read_sparse(&parts, name).map_err(|e| e.to_string())? {
            Partial::Done(_) => return Ok(parts.container()),
            Partial::Need(a, b) => {
                // A read up to the next part held, not over it.
                let end = b.max(a.saturating_add(CHUNK.max(parts.held() / 2))).min(len).min(parts.next_start(a).unwrap_or(len));
                if a >= end {
                    return Err(tl!("the file ends before its data does").to_string());
                }
                if parts.held().saturating_add(end - a) > limit {
                    let mb = (limit >> 20).to_string();
                    return Err(i18n::fmt(
                        "opening it needs more than the web app's {mb} MB, as it isn't cloud-optimised (tiled, with overviews); open it in the desktop app",
                        &[("mb", &mb)],
                    ));
                }
                parts = parts.with(a, slice(file, a, end).await?);
            }
        }
    }
    Err(tl!("too many reads; open it in the desktop app").to_string())
}

/// The bytes `a..b` of a file.
async fn slice(file: &web_sys::File, a: u64, b: u64) -> Result<Vec<u8>, String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let blob = file.slice_with_f64_and_f64(a as f64, b as f64).map_err(err)?;
    let buf = wasm_bindgen_futures::JsFuture::from(blob.array_buffer()).await.map_err(err)?;
    let buf: js_sys::ArrayBuffer = buf.dyn_into().map_err(err)?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}
