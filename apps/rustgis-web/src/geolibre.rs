//! The GeoLibre tools in the browser: geolibre-rust's WASI runner (the `geolibre-wasm` npm
//! package), loaded on demand from jsDelivr and run in an in-memory `/work` folder.

use rustgis_ui_egui::Slot;
use rustgis_ui_egui::rustgis_engine::cmd::gp::{ExternalOutput, ExternalRun};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;

// The runner's version is pinned: the tool forms and the runner must come from the same
// release. The JS module is jsDelivr's ES-module build of the package; the WASI binary is
// passed explicitly, since that build can't find it beside itself.
//
// Trust: this runs code from jsDelivr in the app's origin, only after the user asks for the
// tools (Load GeoLibre tools). A dynamic import can't carry Subresource Integrity, and the
// runner (~25 MB) is too large to ship with the app, so the pinned version and HTTPS are the
// safeguards: the same trust as installing the npm package.
#[wasm_bindgen(inline_js = r#"
const BASE = "https://cdn.jsdelivr.net/npm/geolibre-wasm@1.7.0";
let ready = null;
function tools() {
  if (!ready) {
    ready = (async () => {
      const mod = await import(`${BASE}/tools.mjs/+esm`);
      await mod.initTools(`${BASE}/geolibre-cli.wasm`);
      return mod;
    })();
    // A failed download can be retried.
    ready.catch(() => { ready = null; });
  }
  return ready;
}
// Listing the manifests runs the runner synchronously (seconds of a blocked page), so the list
// is kept in the browser's cache under the runner's versioned URL and read from there on later
// visits. The runner itself loads only when a tool runs.
// The suffix changes when RustGIS reads the list differently.
const MANIFESTS = `${BASE}/manifests.json?v=1`;
async function manifestCache() {
  try { return await caches.open("rustgis-geolibre"); } catch (_) { return null; }
}
export async function geolibre_manifests() {
  const cache = await manifestCache();
  try {
    const hit = cache && await cache.match(MANIFESTS);
    if (hit) {
      const text = await hit.text();
      const list = JSON.parse(text);
      if (Array.isArray(list) && list.length > 0 && typeof list[0]?.id === "string") return text;
    }
  } catch (_) {
    // A damaged entry is listed again below, and the put replaces it.
  }
  const t = await tools();
  const list = await t.listManifests();
  const text = JSON.stringify(list);
  // An empty list isn't kept (it would hide the tools on every later visit), and the lists of
  // earlier runner versions are dropped.
  try {
    if (cache && list.length > 0) {
      await cache.put(MANIFESTS, new Response(text, { headers: { "content-type": "application/json" } }));
      for (const old of await cache.keys()) {
        if (old.url !== MANIFESTS) await cache.delete(old);
      }
    }
  } catch (e) {
    console.warn("RustGIS couldn't keep the GeoLibre tool list; it will be listed again next visit", e);
  }
  return text;
}
export async function geolibre_run(tool, argv, names, datas) {
  const t = await tools();
  const input = {};
  for (let i = 0; i < names.length; i++) input[names[i]] = datas[i];
  const r = await t.runTool(tool, { args: argv, input });
  const out = Object.entries(r.files);
  return { exitCode: r.exitCode, stdout: r.stdout, names: out.map((e) => e[0]), datas: out.map((e) => e[1]) };
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn geolibre_manifests() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn geolibre_run(tool: &str, argv: js_sys::Array, names: js_sys::Array, datas: js_sys::Array) -> Result<JsValue, JsValue>;
}

fn js_error(e: &JsValue) -> String {
    match e.dyn_ref::<js_sys::Error>() {
        Some(err) => String::from(err.message()),
        None => e.as_string().unwrap_or_else(|| format!("{e:?}")),
    }
}

fn get(obj: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn strings(v: &JsValue) -> Vec<String> {
    js_sys::Array::from(v).iter().filter_map(|s| s.as_string()).collect()
}

/// Lists the tools' manifests into `slot`, downloading the runner the first time.
pub fn manifests(slot: Slot<Result<String, String>>, ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        let r = match geolibre_manifests().await {
            Ok(v) => v.as_string().ok_or_else(|| "the GeoLibre runner returned no tool list".to_string()),
            Err(e) => Err(format!("couldn't load the GeoLibre tools: {}", js_error(&e))),
        };
        if let Ok(mut s) = slot.lock() {
            *s = Some(r);
        }
        ctx.request_repaint();
    });
}

/// Runs one tool and puts its new files and printed lines (or its error) in `slot`.
pub fn run(job: ExternalRun, slot: Slot<ExternalOutput>, ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        let argv: js_sys::Array = job.argv.iter().map(|a| JsValue::from_str(a)).collect();
        let names: js_sys::Array = job.files.iter().map(|(n, _)| JsValue::from_str(n)).collect();
        let datas: js_sys::Array = job.files.iter().map(|(_, b)| JsValue::from(js_sys::Uint8Array::from(b.as_slice()))).collect();
        let r = match geolibre_run(&job.tool, argv, names, datas).await {
            Err(e) => Err(js_error(&e)),
            Ok(out) => {
                let lines = strings(&get(&out, "stdout"));
                if get(&out, "exitCode").as_f64().unwrap_or(1.0) != 0.0 {
                    // The tool's error is the end of what it printed.
                    let tail: Vec<&String> = lines.iter().rev().take(12).collect();
                    let msg = tail.into_iter().rev().map(String::as_str).collect::<Vec<_>>().join("\n");
                    Err(if msg.trim().is_empty() { "the tool stopped unexpectedly".to_string() } else { msg })
                } else {
                    let files = strings(&get(&out, "names"))
                        .into_iter()
                        .zip(js_sys::Array::from(&get(&out, "datas")).iter())
                        .map(|(n, d)| (n, js_sys::Uint8Array::new(&d).to_vec()))
                        .collect();
                    Ok((files, lines))
                }
            }
        };
        if let Ok(mut s) = slot.lock() {
            *s = Some(r);
        }
        ctx.request_repaint();
    });
}
