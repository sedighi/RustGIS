//! Cloud data catalogs: Overture Maps (a theme's data where the map is, or the whole world as map
//! tiles) and Source Cooperative (products and their files). Requests go off the frame through the
//! tile cache's downloads (a thread natively, fetch on the web); the files found are added by
//! address through `ui.addUrl` (read in place by range), the tiles through `overture.add`.

use egui::vec2;
use rustgis_engine::render::rustgis_tiles::{cache, overture, s3, sourcecoop};
use serde_json::{Value, json};

use crate::{RustGisApp, Slot};

type Answer = Slot<Result<Vec<u8>, String>>;

/// What is being fetched.
enum Asked {
    /// The latest release, then the tiles (true) or the collection.
    Latest {
        tiles: bool,
    },
    /// A collection's files, for an area.
    Collection {
        area: [f64; 4],
    },
    /// An item; the ones after it, and the files' addresses found so far.
    Item {
        rest: Vec<String>,
        urls: Vec<String>,
    },
    Products,
    Listing,
}

/// The dialogs' choices and the request under way.
pub struct State {
    pub theme: String,
    pub ty: String,
    /// Map tiles of the whole world instead of the features in view.
    pub tiles: bool,
    /// The release, once known (the latest).
    pub release: Option<String>,
    pub account: String,
    pub products: Vec<sourcecoop::Product>,
    /// The product browsed (account, product) and the folder inside it.
    pub product: Option<(String, String)>,
    pub path: String,
    pub listing: s3::Listing,
    waiting: Option<(Asked, Answer)>,
    /// Overture's files being opened (their headers read), every one before any is added.
    opening: Option<(Vec<String>, Vec<Slot<Result<(), String>>>)>,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            theme: "buildings".into(),
            ty: "building".into(),
            tiles: false,
            release: None,
            account: String::new(),
            products: Vec::new(),
            product: None,
            path: String::new(),
            listing: s3::Listing::default(),
            waiting: None,
            opening: None,
            error: None,
            info: None,
        }
    }
}

fn fetch(url: &str) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

/// Opens Overture's files (their headers, in the background); once every one is open, they are
/// added together through `overture.add` (all or none), so a failure adds nothing and trying again
/// adds nothing twice.
fn open_files(app: &mut RustGisApp, urls: Vec<String>) {
    let slots = urls
        .iter()
        .map(|url| {
            let slot: Slot<Result<(), String>> = Slot::default();
            let land = slot.clone();
            rustgis_engine::data::source::open_url(
                url,
                Box::new(move |r| {
                    *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.map(|_| ()).map_err(|e| e.to_string()));
                }),
            );
            slot
        })
        .collect();
    app.ui.catalogs.opening = Some((urls, slots));
}

/// Adds Overture's files once every one is open.
fn poll_opening(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some((_, slots)) = &app.ui.catalogs.opening else { return };
    let done: Vec<Option<Result<(), String>>> = slots.iter().map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone()).collect();
    if done.iter().any(Option::is_none) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    let Some((urls, _)) = app.ui.catalogs.opening.take() else { return };
    let failed = done.into_iter().flatten().find_map(Result::err);
    let st = &app.ui.catalogs;
    let (theme, ty) = (st.theme.clone(), st.ty.clone());
    let r = match failed {
        Some(e) => Err(e),
        None => app.run("overture.add", json!({"theme": theme, "type": ty, "files": urls})).map(|_| {
            app.ui.catalogs.info = Some(crate::i18n::fmt(
                "Reading {n} of Overture's {ty} files where the map is: {attribution}",
                &[("n", &urls.len().to_string()), ("ty", &ty), ("attribution", overture::attribution(&theme))],
            ));
        }),
    };
    match r {
        Ok(()) => app.map.invalidate(),
        Err(e) => app.ui.catalogs.error = Some(e),
    }
}

/// Lands the answers and moves Overture's steps along.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    poll_opening(app, ctx);
    let Some((_, slot)) = &app.ui.catalogs.waiting else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((asked, _)) = app.ui.catalogs.waiting.take() else { return };
    // A release that has no such collection any more was retired: the next Add reads the catalog
    // again. Other failures (the network, a file) keep it.
    let retired = matches!(asked, Asked::Collection { .. }) && r.as_ref().err().is_some_and(|e| cache::http_status(e) == Some(404));
    let done = match asked {
        Asked::Latest { tiles } => r.and_then(|b| overture::parse_latest(&b)).and_then(|release| {
            app.ui.catalogs.release = Some(release);
            overture_next(app, tiles)
        }),
        Asked::Collection { area } => r.and_then(|b| overture::parse_collection(&b)).and_then(|files| {
            let ty = app.ui.catalogs.ty.clone();
            let hits: Vec<String> = overture::files_in(&files, area).iter().map(|f| f.item.clone()).collect();
            if hits.is_empty() {
                return Err(crate::i18n::fmt("Overture has no {ty} data where the map is", &[("ty", &ty)]));
            }
            if hits.len() > overture::MAX_FILES {
                return Err(crate::i18n::fmt(
                    "The map's view takes {n} of Overture's {ty} files (up to {max}): zoom in, or add the theme as map tiles",
                    &[("n", &hits.len().to_string()), ("ty", &ty), ("max", &overture::MAX_FILES.to_string())],
                ));
            }
            let mut rest = hits.into_iter();
            if let Some(first) = rest.next() {
                app.ui.catalogs.waiting = Some((Asked::Item { rest: rest.collect(), urls: Vec::new() }, fetch(&first)));
            }
            Ok(())
        }),
        // Every file's address first, then every file opened: nothing is added until all are.
        Asked::Item { mut rest, mut urls } => r.and_then(|b| overture::parse_item(&b)).map(|url| {
            urls.push(url);
            if rest.is_empty() {
                open_files(app, urls);
            } else {
                let next = rest.remove(0);
                app.ui.catalogs.waiting = Some((Asked::Item { rest, urls }, fetch(&next)));
            }
        }),
        Asked::Products => r.and_then(|b| sourcecoop::parse_products(&b)).map(|p| {
            if p.is_empty() {
                app.ui.catalogs.info = Some(tl!("No products").to_string());
            }
            app.ui.catalogs.products = p;
        }),
        // A next page is added to what is listed already.
        Asked::Listing => r.and_then(|b| s3::parse_list(&b)).map(|l| {
            let shown = &mut app.ui.catalogs.listing;
            shown.prefixes.extend(l.prefixes);
            shown.objects.extend(l.objects);
            shown.next = l.next;
        }),
    };
    match done {
        Ok(()) => app.map.invalidate(),
        Err(e) => {
            if retired {
                app.ui.catalogs.release = None;
            }
            app.ui.catalogs.error = Some(e);
        }
    }
}

/// Overture's next step once the release is known: the tiles, or the collection for the view.
fn overture_next(app: &mut RustGisApp, tiles: bool) -> Result<(), String> {
    let st = &app.ui.catalogs;
    let release = st.release.clone().ok_or("the release isn't known")?;
    let (theme, ty) = overture::theme_type(&st.theme, Some(&st.ty))?;
    if tiles {
        app.run("overture.add", json!({"theme": theme, "release": release, "tiles": true}))?;
        app.ui.catalogs.info = Some(crate::i18n::fmt(
            "Added Overture's {theme} as map tiles ({release}; some themes show only zoomed in): {attribution}",
            &[("theme", theme), ("release", &release), ("attribution", overture::attribution(theme))],
        ));
        return Ok(());
    }
    let area = rustgis_engine::cmd::catalogs::area(&app.session, &Value::Null)?;
    app.ui.catalogs.waiting = Some((Asked::Collection { area }, fetch(&overture::collection_url(&release, theme, ty))));
    Ok(())
}

fn status(app: &RustGisApp, ui: &mut egui::Ui) {
    if app.ui.catalogs.waiting.is_some() || app.ui.catalogs.opening.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Asking the catalog…"));
        });
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    if let Some(e) = &app.ui.catalogs.error {
        ui.label(egui::RichText::new(e).color(t.red));
    }
    if let Some(i) = &app.ui.catalogs.info {
        ui.label(egui::RichText::new(i).weak());
    }
}

/// Overture Maps: a theme's type where the map is (read in place from its GeoParquet files), or the
/// whole theme as map tiles.
pub fn overture(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("Overture Maps' open data: buildings, places, transportation, administrative divisions, base layers and addresses. Where the map is, its features are read in place from Overture's GeoParquet files (zoom to a city or smaller); the whole world comes as map tiles.")).weak());
    let busy = app.ui.catalogs.waiting.is_some() || app.ui.catalogs.opening.is_some();
    egui::Grid::new("overture").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.catalogs;
        ui.label(tl!("Theme"));
        egui::ComboBox::from_id_salt("ov_theme").selected_text(st.theme.clone()).width(260.0).show_ui(ui, |ui| {
            for (theme, types) in overture::THEMES {
                if ui.selectable_label(st.theme == *theme, *theme).clicked() {
                    st.theme = (*theme).to_string();
                    st.ty = types.first().copied().unwrap_or_default().to_string();
                }
            }
        });
        ui.end_row();
        ui.label(tl!("Type"));
        let types = overture::THEMES.iter().find(|(t, _)| *t == st.theme).map(|(_, y)| *y).unwrap_or(&[]);
        egui::ComboBox::from_id_salt("ov_type").selected_text(st.ty.clone()).width(260.0).show_ui(ui, |ui| {
            for ty in types {
                ui.selectable_value(&mut st.ty, (*ty).to_string(), *ty);
            }
        });
        ui.end_row();
        ui.label(tl!("Add"));
        ui.vertical(|ui| {
            ui.radio_value(&mut st.tiles, false, tl!("The features where the map is"));
            ui.radio_value(&mut st.tiles, true, tl!("Map tiles of the whole world"));
        });
        ui.end_row();
        if let Some(r) = &st.release {
            ui.label(tl!("Release"));
            ui.label(r);
            ui.end_row();
        }
    });
    if ui.add_enabled(!busy, egui::Button::new(tl!("Add to Map"))).clicked() {
        app.ui.catalogs.error = None;
        app.ui.catalogs.info = None;
        let tiles = app.ui.catalogs.tiles;
        let r = match app.ui.catalogs.release.is_some() {
            true => overture_next(app, tiles),
            false => {
                app.ui.catalogs.waiting = Some((Asked::Latest { tiles }, fetch(&overture::catalog_url())));
                Ok(())
            }
        };
        if let Err(e) = r {
            app.ui.catalogs.error = Some(e);
        }
    }
    status(app, ui);
}

/// Lists a level of the product browsed: from its start, or (`more`) its next page, added to what
/// is shown.
fn list(app: &mut RustGisApp, more: bool) {
    let Some((account, product)) = app.ui.catalogs.product.clone() else { return };
    let token = if more { app.ui.catalogs.listing.next.clone() } else { None };
    if more && token.is_none() {
        return;
    }
    match sourcecoop::list_url(&account, &product, &app.ui.catalogs.path, token.as_deref()) {
        Ok(url) => {
            app.ui.catalogs.error = None;
            if !more {
                app.ui.catalogs.listing = s3::Listing::default();
            }
            app.ui.catalogs.waiting = Some((Asked::Listing, fetch(&url)));
        }
        Err(e) => app.ui.catalogs.error = Some(e),
    }
}

/// A size, readable.
fn size_text(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

/// Source Cooperative: the featured products (or an account's), a product's folders and files; a
/// file is added by its address.
pub fn sourcecoop(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("Open data products on Source Cooperative. GeoParquet, FlatGeobuf, cloud-optimised GeoTIFFs and PMTiles are read in place; other files are downloaded.")).weak());
    let busy = app.ui.catalogs.waiting.is_some();
    let mut ask = None;
    ui.horizontal(|ui| {
        ui.label(tl!("Account"));
        ui.add(egui::TextEdit::singleline(&mut app.ui.catalogs.account).hint_text(tl!("featured products")).desired_width(220.0));
        if ui.add_enabled(!busy, egui::Button::new(tl!("List Products"))).clicked() {
            let a = app.ui.catalogs.account.trim().to_string();
            ask = Some(if a.is_empty() { Ok(sourcecoop::featured_url()) } else { sourcecoop::account_url(&a) });
        }
    });
    if let Some(url) = ask {
        app.ui.catalogs.error = None;
        app.ui.catalogs.info = None;
        match url {
            Ok(url) => {
                app.ui.catalogs.product = None;
                app.ui.catalogs.waiting = Some((Asked::Products, fetch(&url)));
            }
            Err(e) => app.ui.catalogs.error = Some(e),
        }
    }
    let mut open_product = None;
    let mut into = None;
    let mut up = false;
    let mut add = None;
    let mut more = false;
    match app.ui.catalogs.product.clone() {
        None => {
            egui::ScrollArea::vertical().max_height(300.0).id_salt("sc_products").show(ui, |ui| {
                for p in &app.ui.catalogs.products {
                    let r = ui.add(egui::Button::new(&p.title).wrap().min_size(vec2(ui.available_width(), 0.0)));
                    let r = r.on_hover_text(format!("{}/{}\n{}", p.account, p.id, p.description));
                    if r.clicked() {
                        open_product = Some((p.account.clone(), p.id.clone()));
                    }
                }
            });
        }
        Some((account, product)) => {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(tl!("Products"))).clicked() {
                    app.ui.catalogs.product = None;
                }
                if ui.add_enabled(!busy && !app.ui.catalogs.path.is_empty(), egui::Button::new(tl!("Up"))).clicked() {
                    up = true;
                }
                ui.label(egui::RichText::new(format!("{account}/{product}/{}", app.ui.catalogs.path)).monospace());
            });
            let base = format!("{product}/");
            egui::ScrollArea::vertical().max_height(300.0).id_salt("sc_files").show(ui, |ui| {
                for p in &app.ui.catalogs.listing.prefixes {
                    let label = p.strip_prefix(&base).unwrap_or(p).strip_prefix(app.ui.catalogs.path.as_str()).unwrap_or(p);
                    // A folder: the folder icon (drawn) and its name.
                    let clicked = ui
                        .horizontal(|ui| {
                            let t = crate::theme::Tokens::get(ui.ctx());
                            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                            crate::icons::paint(ui.painter(), r, "open", t.icon, t.accent);
                            ui.selectable_label(false, label).clicked()
                        })
                        .inner;
                    if clicked {
                        into = Some(p.strip_prefix(&base).unwrap_or(p).to_string());
                    }
                }
                for o in &app.ui.catalogs.listing.objects {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new(tl!("Add")).small()).clicked() {
                            add = Some((account.clone(), o.key.clone()));
                        }
                        let name = o.key.rsplit('/').next().unwrap_or(&o.key);
                        ui.label(name);
                        ui.label(egui::RichText::new(size_text(o.size)).small().weak());
                    });
                }
            });
            // Past the first page (1000 entries): what is shown, and the next page on request.
            if app.ui.catalogs.listing.next.is_some() {
                ui.horizontal(|ui| {
                    let shown = app.ui.catalogs.listing.prefixes.len() + app.ui.catalogs.listing.objects.len();
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt("{n} shown, more in the folder", &[("n", &shown.to_string())])).small().weak(),
                    );
                    if ui.add_enabled(!busy, egui::Button::new(tl!("More"))).clicked() {
                        more = true;
                    }
                });
            }
        }
    }
    if let Some(p) = open_product {
        app.ui.catalogs.product = Some(p);
        app.ui.catalogs.path.clear();
        list(app, false);
    }
    if let Some(path) = into {
        app.ui.catalogs.path = path;
        list(app, false);
    }
    if more {
        list(app, true);
    }
    if up {
        let p = app.ui.catalogs.path.trim_end_matches('/');
        app.ui.catalogs.path = p.rsplit_once('/').map(|(a, _)| format!("{a}/")).unwrap_or_default();
        list(app, false);
    }
    if let Some((account, key)) = add {
        app.ui.catalogs.error = None;
        match sourcecoop::file_url(&account, &key) {
            Ok(url) => {
                let stem = key.rsplit('/').next().unwrap_or(&key);
                let stem = stem.split_once('.').map(|(a, _)| a).unwrap_or(stem).to_string();
                if let Err(e) = app.run("ui.addUrl", json!({"url": url, "name": stem})) {
                    app.ui.catalogs.error = Some(e);
                }
            }
            Err(e) => app.ui.catalogs.error = Some(e),
        }
    }
    status(app, ui);
}
