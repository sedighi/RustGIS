//! Sprites: the icons and fill patterns a style draws, packed into one picture with an index.
//!
//! A style's `sprite` address names two files, `<address>.json` (where each image is in the
//! picture, its pixel ratio and whether it is an SDF icon) and `<address>.png`, and their `@2x`
//! versions for high-density screens. Both come through the tile cache; images are cut out of
//! the picture the first time they are asked for, and SDF icons are coloured as they are drawn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tiny_skia::{IntRect, Pixmap};

use crate::cache::{self, Payload, State, Want};
use crate::style::Rgba;

/// Where one image is in the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    ratio: f32,
    sdf: bool,
}

/// A loaded sprite.
pub struct Sprite {
    sheet: Pixmap,
    index: HashMap<String, Entry>,
    /// Images cut out so far, by name and (for SDF icons) colour: looked up by `&str`, so a
    /// hit allocates nothing.
    cut: Mutex<HashMap<String, HashMap<Option<Rgba>, Arc<Pixmap>>>>,
}

/// An image from a sprite.
#[derive(Clone)]
pub struct Image {
    pub pixmap: Arc<Pixmap>,
    /// Pixels per logical point it was drawn for (2 in an `@2x` sprite).
    pub ratio: f32,
}

/// The most images a sprite index lists.
const MAX_IMAGES: usize = 10_000;
/// The largest side of one image (icons and patterns are tens of pixels).
const MAX_SIDE: f64 = 1024.0;
/// The most bytes of cut images kept.
const MAX_CUT_BYTES: usize = 32 << 20;

impl Sprite {
    /// Reads a sprite from its index and picture.
    pub fn new(index: &serde_json::Value, sheet: Pixmap) -> Sprite {
        let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_f64);
        let index = index
            .as_object()
            .into_iter()
            .flatten()
            .take(MAX_IMAGES)
            .filter_map(|(name, v)| {
                let (x, y, w, h) = (num(v, "x")?, num(v, "y")?, num(v, "width")?, num(v, "height")?);
                if [x, y].iter().any(|n| !(n.is_finite() && *n >= 0.0 && *n <= 16_384.0))
                    || ![w, h].iter().all(|n| n.is_finite() && *n >= 1.0 && *n <= MAX_SIDE)
                {
                    return None;
                }
                let ratio = num(v, "pixelRatio").filter(|r| r.is_finite() && *r > 0.0 && *r <= 8.0).unwrap_or(1.0) as f32;
                let sdf = v.get("sdf").and_then(serde_json::Value::as_bool).unwrap_or(false);
                Some((name.clone(), Entry { x: x as u32, y: y as u32, w: w as u32, h: h as u32, ratio, sdf }))
            })
            .collect();
        Sprite { sheet, index, cut: Mutex::default() }
    }

    fn entry(&self, name: &str) -> Option<(&str, Entry)> {
        if let Some((k, e)) = self.index.get_key_value(name) {
            return Some((k.as_str(), *e));
        }
        // A style with several sprites names their images `<sprite id>:<name>`.
        let (_, bare) = name.split_once(':')?;
        self.index.get_key_value(bare).map(|(k, e)| (k.as_str(), *e))
    }

    /// The image `name`, if the sprite has it. An SDF icon is drawn in `color` (black when
    /// none is given).
    pub fn image(&self, name: &str, color: Option<Rgba>) -> Option<Image> {
        let (key, e) = self.entry(name)?;
        let color = e.sdf.then(|| color.unwrap_or([0, 0, 0, 255]));
        if let Some(p) = self.cut.lock().unwrap_or_else(|p| p.into_inner()).get(key).and_then(|by_color| by_color.get(&color)) {
            return Some(Image { pixmap: p.clone(), ratio: e.ratio });
        }
        // Cut outside the lock. Wholly inside the picture (cutting would otherwise clip a bad entry to a part).
        if e.x.checked_add(e.w)? > self.sheet.width() || e.y.checked_add(e.h)? > self.sheet.height() {
            return None;
        }
        let mut p = self.sheet.clone_rect(IntRect::from_xywh(e.x as i32, e.y as i32, e.w, e.h)?)?;
        if let Some(c) = color {
            sdf_fill(&mut p, c);
        }
        let p = Arc::new(p);
        let mut cut = self.cut.lock().unwrap_or_else(|p| p.into_inner());
        // Kept within a byte budget: an SDF icon cut in many colours, or a style naming
        // thousands of images, starts the cache again rather than growing it.
        let bytes: usize = cut.values().flat_map(HashMap::values).map(|p| p.data().len()).sum();
        if bytes + p.data().len() > MAX_CUT_BYTES {
            cut.clear();
        }
        cut.entry(key.to_string()).or_default().insert(color, p.clone());
        Some(Image { pixmap: p, ratio: e.ratio })
    }
}

/// Colours an SDF icon: its alpha is a distance from the edge (0.75 at the edge, more inside),
/// which becomes `c`, smoothed over a pixel.
fn sdf_fill(p: &mut Pixmap, c: Rgba) {
    const EDGE: f32 = 0.75;
    const SMOOTH: f32 = 0.06;
    for px in p.data_mut().as_chunks_mut::<4>().0 {
        let d = f32::from(px[3]) / 255.0;
        let t = ((d - (EDGE - SMOOTH)) / (2.0 * SMOOTH)).clamp(0.0, 1.0);
        let a = t * t * (3.0 - 2.0 * t) * f32::from(c[3]) / 255.0;
        for (i, ch) in c.iter().take(3).enumerate() {
            px[i] = (f32::from(*ch) * a).round() as u8;
        }
        px[3] = (a * 255.0).round() as u8;
    }
}

pub enum Load {
    Ready(Arc<Sprite>),
    Pending,
    Failed,
}

fn sprites() -> &'static Mutex<HashMap<String, Arc<Sprite>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<Sprite>>>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// Forgets the decoded sprite sheets, so they are read again (with [`crate::cache::clear`]).
pub(crate) fn forget() {
    sprites().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// `<address><suffix>`, the suffix going before any query string.
fn with_suffix(base: &str, suffix: &str) -> String {
    match base.split_once('?') {
        Some((path, query)) => format!("{path}{suffix}?{query}"),
        None => format!("{base}{suffix}"),
    }
}

/// The sprite at `base` (an address without `.json`/`.png`), at high density when `hi_dpi`
/// and the style has one, else at 1×.
pub fn load(base: &str, hi_dpi: bool) -> Load {
    if hi_dpi {
        match load_one(base, "@2x") {
            Load::Failed => {}
            other => return other,
        }
    }
    load_one(base, "")
}

fn load_one(base: &str, density: &str) -> Load {
    let key = format!("{base}|{density}");
    if let Some(s) = sprites().lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Load::Ready(s.clone());
    }
    let index = cache::get(&with_suffix(base, &format!("{density}.json")), Want::Json);
    let sheet = cache::get(&with_suffix(base, &format!("{density}.png")), Want::Image);
    match (index, sheet) {
        (State::Ready(i), State::Ready(p)) => match (&*i, &*p) {
            (Payload::Json(index), Payload::Image(sheet)) => {
                let s = Arc::new(Sprite::new(index, sheet.clone()));
                let mut all = sprites().lock().unwrap_or_else(|p| p.into_inner());
                if all.len() > 16 {
                    all.clear();
                }
                all.insert(key, s.clone());
                Load::Ready(s)
            }
            _ => Load::Failed,
        },
        (State::Failed(_), _) | (_, State::Failed(_)) => Load::Failed,
        _ => Load::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> Pixmap {
        let mut p = Pixmap::new(8, 4).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        p
    }

    #[test]
    fn forgetting_drops_the_decoded_sheets() {
        let key = "forget-test|".to_string();
        let sprite = Sprite { sheet: sheet(), index: Default::default(), cut: Default::default() };
        sprites().lock().unwrap().insert(key.clone(), Arc::new(sprite));
        forget();
        assert!(!sprites().lock().unwrap().contains_key(&key));
    }

    #[test]
    fn cuts_images_out() {
        let index = serde_json::json!({
            "dot": {"x": 0, "y": 0, "width": 4, "height": 4, "pixelRatio": 2},
            "sdf": {"x": 4, "y": 0, "width": 4, "height": 4, "pixelRatio": 1, "sdf": true},
            "bad": {"x": 6, "y": 0, "width": 40, "height": 4},
            "huge": {"x": 0, "y": 0, "width": 4096, "height": 4},
            "junk": {"x": -1}
        });
        let s = Sprite::new(&index, sheet());
        let dot = s.image("dot", None).unwrap();
        assert_eq!((dot.pixmap.width(), dot.ratio), (4, 2.0));
        // Several sprites: `<id>:<name>`.
        assert!(s.image("base:dot", None).is_some());
        assert!(s.image("bad", None).is_none() && s.image("junk", None).is_none() && s.image("nope", None).is_none());
        assert!(s.image("huge", None).is_none(), "over the largest side");
        // An SDF icon takes the colour asked for (full alpha is well inside the edge).
        let blue = s.image("sdf", Some([0, 0, 255, 255])).unwrap();
        let px = blue.pixmap.pixel(1, 1).unwrap();
        assert_eq!((px.red(), px.blue(), px.alpha()), (0, 255, 255));
    }

    #[test]
    fn suffixes_go_before_the_query() {
        assert_eq!(with_suffix("https://a/sprite?key=1", "@2x.png"), "https://a/sprite@2x.png?key=1");
        assert_eq!(with_suffix("https://a/sprite", ".json"), "https://a/sprite.json");
    }
}
