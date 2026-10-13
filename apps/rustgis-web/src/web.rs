//! The browser shell: file picker, downloads, drag-and-drop, the unsaved-changes guard and the
//! eframe web runner.

use std::cell::Cell;
use std::rc::Rc;

use rustgis_ui_egui::{Inbox, RustGisApp, Services};
use wasm_bindgen::JsCast as _;

const DATA_EXTS: &[&str] = &[
    "geojson",
    "json",
    "csv",
    "tsv",
    "txt",
    "zip",
    "kml",
    "kmz",
    "gpx",
    "gpkg",
    "parquet",
    "geoparquet",
    "fgb",
    "mbtiles",
    "tif",
    "tiff",
    "asc",
];
const CANVAS_ID: &str = "rustgis_canvas";
const LOADING_ID: &str = "rustgis_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let dirty = Rc::new(Cell::new(false));
        if let Err(e) = guard_unload(dirty.clone()) {
            log::error!("no unsaved-changes guard: {e}");
        }
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    let inbox: Inbox = Inbox::default();
                    let font_inbox: Inbox = Inbox::default();
                    let q = query();
                    let mut app = RustGisApp::with_sample().with_services(services(inbox.clone(), font_inbox, cc.egui_ctx.clone()));
                    // Settings › Start-up, unless `?blank`, `?projection=EPSG:3010` (a preset id,
                    // `mercator`, `globe`) or `?extent=minLon,minLat,maxLon,maxLat` says otherwise
                    // for this visit.
                    let start = rustgis_ui_egui::Start {
                        blank: q.trim_start_matches('?').split('&').any(|kv| kv == "blank").then_some(true),
                        projection: param(&q, "projection").filter(|c| !c.is_empty()),
                        extent: param(&q, "extent").filter(|c| !c.is_empty()),
                    };
                    if let Err(e) = app.start(&start) {
                        log::warn!("start-up: {e}");
                    }
                    // `?lang=de` picks the interface language for this visit.
                    if let Some(code) = q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("lang=")) {
                        match rustgis_ui_egui::i18n::Lang::from_code(code) {
                            Some(l) => app.ui.language = l.code().to_string(),
                            None => log::warn!("?lang={code}: no such interface language; following the browser's"),
                        }
                    }
                    // `?registry=<url>` opens the Plugin Manager on another plugin registry for this
                    // visit (a pull request's preview, say); installing still asks as usual.
                    if let Some(url) = param(&q, "registry").filter(|u| !u.is_empty())
                        && let Err(e) = app.run("ui.plugins", serde_json::json!({"open": true, "registry": url, "tab": "browse"}))
                    {
                        log::warn!("?registry={url}: {e}");
                    }
                    Ok(Box::new(WebShell { app, inbox, dirty }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    el.set_inner_html(&format!("<p>RustGIS failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                }
            }
        }
    });
}

/// A query parameter's value, percent-decoded.
fn param(q: &str, key: &str) -> Option<String> {
    let raw = q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?;
    js_sys::decode_uri_component(&raw.replace('+', " ")).ok().and_then(|s| s.as_string())
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Reads dropped files asynchronously (browsers can't read them synchronously) into the inbox,
/// and mirrors the unsaved-changes flag for the `beforeunload` guard.
struct WebShell {
    app: RustGisApp,
    inbox: Inbox,
    dirty: Rc<Cell<bool>>,
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for f in dropped {
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped.geojson".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
                }
            });
        }
        self.app.logic(ctx);
        self.dirty.set(self.app.session.dirty);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Downloads `url`, or the byte range it asks for (a PMTiles archive's part: see
/// `rustgis_tiles::cache::split_range`).
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let (url, range) = rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache::split_range(url);
    let promise = match range {
        Some((a, b)) => {
            let init = web_sys::RequestInit::new();
            let headers = web_sys::Headers::new().map_err(err)?;
            headers.set("Range", &format!("bytes={a}-{b}")).map_err(err)?;
            init.set_headers(&headers);
            window.fetch_with_str_and_init(url, &init)
        }
        None => window.fetch_with_str(url),
    };
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(err)?.dyn_into().map_err(err)?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    // A range asked for and the whole file answered (200): fine for a range from the start that
    // holds the whole file (a small archive), checked once it's read; else no ranges.
    let whole = range.is_some() && resp.status() != 206;
    if range.is_some_and(|(a, _)| whole && a > 0) {
        return Err(format!("{url}: the server doesn't answer byte-range requests"));
    }
    // Fonts are at most about 10 MB and plugins 64 MB; refuse anything over 64 MB, by header when there is one and
    // by the downloaded size otherwise.
    if let Ok(Some(len)) = resp.headers().get("content-length").map(|v| v.and_then(|s| s.parse::<u64>().ok()))
        && len > 64 << 20
    {
        return Err(format!("{len} bytes is too large"));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(err)?).await.map_err(err)?;
    let bytes = js_sys::Uint8Array::new(&buf);
    if bytes.length() > 64 << 20 {
        return Err(format!("{} bytes is too large", bytes.length()));
    }
    if let Some((a, b)) = range.filter(|_| whole)
        && u64::from(bytes.length()) > b - a + 1
    {
        return Err(format!("{url}: the server doesn't answer byte-range requests"));
    }
    Ok(bytes.to_vec())
}

/// Makes an HTTP request (the AI assistant's calls to its model), returning the response of any
/// status.
async fn http(req: rustgis_ui_egui::rustgis_assistant::HttpRequest) -> Result<rustgis_ui_egui::rustgis_assistant::HttpResponse, String> {
    use rustgis_ui_egui::rustgis_assistant::provider::{MAX_RESPONSE, allowed_url};
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    if !allowed_url(&req.url) {
        return Err(format!("{}: only HTTPS addresses, or plain HTTP on this computer or network, are allowed", req.url));
    }
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method(req.method);
    // The API key is never carried on to another address.
    init.set_redirect(web_sys::RequestRedirect::Error);
    let headers = web_sys::Headers::new().map_err(err)?;
    for (k, v) in &req.headers {
        headers.set(k, v).map_err(err)?;
    }
    init.set_headers(&headers);
    if let Some(body) = &req.body {
        init.set_body(&wasm_bindgen::JsValue::from_str(body));
    }
    // A failed fetch here is usually the provider refusing browser requests (CORS) or no network.
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(&req.url, &init))
        .await
        .map_err(|_| "couldn't reach the provider (offline, or it doesn't accept requests from web pages)".to_string())?
        .dyn_into()
        .map_err(err)?;
    let status = resp.status();
    // Too large by its header: refused before it is read.
    if let Ok(Some(len)) = resp.headers().get("content-length").map(|v| v.and_then(|s| s.parse::<u64>().ok()))
        && len > MAX_RESPONSE as u64
    {
        return Err(format!("{len} bytes is too large"));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(err)?).await.map_err(err)?;
    let bytes = js_sys::Uint8Array::new(&buf);
    if bytes.length() as usize > MAX_RESPONSE {
        return Err(format!("{} bytes is too large", bytes.length()));
    }
    Ok(rustgis_ui_egui::rustgis_assistant::HttpResponse { status, body: bytes.to_vec() })
}

fn services(inbox: Inbox, font_inbox: Inbox, ctx: egui::Context) -> Services {
    // Map tiles come through the browser's fetch (the app repaints when they arrive).
    rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache::set_fetcher(|url, done| {
        wasm_bindgen_futures::spawn_local(async move { done(fetch_bytes(&url).await) });
    });
    let open_inbox = inbox.clone();
    let font_ctx = ctx.clone();
    let (gl_list_ctx, gl_run_ctx) = (ctx.clone(), ctx.clone());
    let font_sink = font_inbox.clone();
    let fetch_ctx = ctx.clone();
    let http_ctx = ctx.clone();
    Services {
        load_pref: Some(Box::new(|key: &str| storage().and_then(|s| s.get_item(&format!("rustgis.{key}")).ok().flatten()))),
        save_pref: Some(Box::new(|key: &str, value: &str| {
            if let Some(s) = storage() {
                let _ = s.set_item(&format!("rustgis.{key}"), value);
            }
        })),
        fetch_font: Some(Box::new(move |url: &str| {
            let (req_url, inbox, ctx) = (url.to_string(), font_sink.clone(), font_ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let mut res = fetch_bytes(&req_url).await;
                if res.is_err() && req_url.contains("Vazirmatn") {
                    res = fetch_bytes("https://cdn.jsdelivr.net/gh/rastikerdar/vazirmatn@v33.003/fonts/ttf/Vazirmatn-Regular.ttf").await;
                }
                match res {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((req_url, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        log::error!("couldn't load font {req_url}: {e}");
                        // Empty bytes tell the app the download failed.
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((req_url, Vec::new()));
                        ctx.request_repaint();
                    }
                }
            });
        })),
        font_inbox: Some(font_inbox),
        open_async: Some(Box::new(move |purpose: &str| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            // A file picked for Add Delimited Text carries its dialog's options.
            let mark = if purpose == "text" { rustgis_ui_egui::DELIMITED_PICK } else { "" };
            let dialog = if purpose == "project" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS project", &["rgis", "rgisx"])
            } else if purpose == "item" {
                rfd::AsyncFileDialog::new().add_filter(
                    "RustGIS items",
                    &["rgismap", "rgismpk", "rgispkg", "rgislyr", "rgisstyle", "rgistemplate", "rgistask", "rgis", "rgisx"],
                )
            } else if purpose == "layout" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS layout", &["rgislayout"])
            } else if purpose == "plugin" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS plugin", &["rgplugin"])
            } else if purpose == "text" {
                rfd::AsyncFileDialog::new().add_filter("Delimited text", &["csv", "tsv", "txt"])
            } else if purpose == "picture" {
                rfd::AsyncFileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg"])
            } else {
                rfd::AsyncFileDialog::new().add_filter("Vector and raster data", DATA_EXTS)
            };
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    return;
                };
                let bytes = file.read().await;
                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((format!("{mark}{}", file.file_name()), bytes));
                ctx.request_repaint();
            });
        })),
        download: Some(Box::new(|name: &str, bytes: Vec<u8>| {
            if let Err(e) = download(name, &bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
        geolibre_manifests: Some(Box::new(move |slot| crate::geolibre::manifests(slot, gl_list_ctx.clone()))),
        geolibre_run: Some(Box::new(move |job, slot| crate::geolibre::run(job, slot, gl_run_ctx.clone()))),
        http: Some(Box::new(move |req, slot| {
            let ctx = http_ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let r = http(req).await;
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                ctx.request_repaint();
            });
        })),
        fetch: Some(Box::new(move |url: &str, slot| {
            let (url, ctx) = (url.to_string(), fetch_ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let r = fetch_bytes(&url).await.map_err(|e| format!("{url}: {e}"));
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                ctx.request_repaint();
            });
        })),
    }
}

/// Saves bytes through a temporary `<a download>` link.
fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let props = web_sys::BlobPropertyBag::new();
    props.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    // Some browsers only follow links that are in the document.
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(err)?;
    a.click();
    a.remove();
    // Browsers may start the download after `click` returns, so free the URL a little later.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let window = web_sys::window().ok_or("no window")?;
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(err)?;
    Ok(())
}

/// Closing, reloading or leaving the tab with unsaved changes asks first.
fn guard_unload(dirty: Rc<Cell<bool>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
        if dirty.get() {
            e.prevent_default();
            e.set_return_value("unsaved");
        }
    });
    window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    on_unload.forget();
    Ok(())
}
