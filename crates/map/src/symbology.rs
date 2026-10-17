//! Symbols, renderers (single symbol, unique values, graduated colours, graduated symbols) and
//! labeling. A renderer maps each feature to a [`Symbol`] and describes its legend.

use std::collections::BTreeMap;

use rustgis_data::{Dataset, FieldType, GeometryType};
use serde::{Deserialize, Serialize};

use crate::color::{Ramp, Rgba};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Marker {
    #[default]
    Circle,
    Square,
    Triangle,
    Diamond,
    Star,
}

impl Marker {
    pub const ALL: [Marker; 5] = [Marker::Circle, Marker::Square, Marker::Triangle, Marker::Diamond, Marker::Star];
    pub fn label(self) -> &'static str {
        match self {
            Marker::Circle => "Circle",
            Marker::Square => "Square",
            Marker::Triangle => "Triangle",
            Marker::Diamond => "Diamond",
            Marker::Star => "Star",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub fill: Rgba,
    pub stroke: Rgba,
    /// Outline / line width in points.
    pub stroke_width: f32,
    /// Marker size in points.
    pub size: f32,
    pub marker: Marker,
}

/// Default palette for new layers (cycled by layer id).
const LAYER_COLORS: [[u8; 3]; 8] = [
    [0x4A, 0x90, 0xD9],
    [0xE0, 0x7B, 0x39],
    [0x5B, 0xB0, 0x6E],
    [0xC2, 0x55, 0x9B],
    [0xE6, 0xB8, 0x3A],
    [0x7D, 0x6B, 0xD1],
    [0x3C, 0xB1, 0xB8],
    [0xB9, 0x5F, 0x4A],
];

impl Symbol {
    pub fn default_for(g: GeometryType, seed: u32) -> Symbol {
        let c = LAYER_COLORS.get(seed as usize % LAYER_COLORS.len()).copied().unwrap_or([0x4A, 0x90, 0xD9]);
        let base = Rgba::rgb(c[0], c[1], c[2]);
        match g {
            GeometryType::Point => {
                Symbol { fill: base, stroke: Rgba::rgb(255, 255, 255), stroke_width: 1.0, size: 7.0, marker: Marker::Circle }
            }
            GeometryType::Polyline => Symbol { fill: base, stroke: base, stroke_width: 1.6, size: 0.0, marker: Marker::Circle },
            _ => Symbol { fill: base.with_alpha(170), stroke: base.shade(0.6), stroke_width: 0.7, size: 0.0, marker: Marker::Circle },
        }
    }

    pub fn with_fill(mut self, c: Rgba, g: GeometryType) -> Symbol {
        match g {
            GeometryType::Polyline => {
                self.fill = c;
                self.stroke = c;
            }
            GeometryType::Polygon => {
                self.fill = c.with_alpha(self.fill.a().max(150));
                self.stroke = c.shade(0.6);
            }
            _ => self.fill = c,
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassMethod {
    #[default]
    NaturalBreaks,
    EqualInterval,
    Quantile,
}

impl ClassMethod {
    pub const ALL: [ClassMethod; 3] = [ClassMethod::NaturalBreaks, ClassMethod::EqualInterval, ClassMethod::Quantile];
    pub fn label(self) -> &'static str {
        match self {
            ClassMethod::NaturalBreaks => "Natural Breaks (Jenks)",
            ClassMethod::EqualInterval => "Equal Interval",
            ClassMethod::Quantile => "Quantile",
        }
    }
    pub fn from_name(s: &str) -> Option<ClassMethod> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "naturalbreaks" | "natural_breaks" | "jenks" | "natural breaks" => ClassMethod::NaturalBreaks,
            "equalinterval" | "equal_interval" | "equal interval" => ClassMethod::EqualInterval,
            "quantile" => ClassMethod::Quantile,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Renderer {
    Single(Symbol),
    Unique {
        field: String,
        classes: Vec<(String, Symbol)>,
        other: Symbol,
        ramp: Ramp,
        /// Legend labels set by hand, by class value; a class without one shows its value. A value
        /// listed twice (a style import can do that) shares one label, as it shares one drawn
        /// symbol: features take their value's first class (`symbol_for`).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        class_labels: BTreeMap<String, String>,
        /// The legend label of `other`, when set by hand (otherwise `<all other values>`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        other_label: Option<String>,
    },
    /// Upper bounds of each class (ascending) and one symbol per class.
    Graduated {
        field: String,
        breaks: Vec<f64>,
        symbols: Vec<Symbol>,
        ramp: Ramp,
        method: ClassMethod,
        min: f64,
        /// Legend labels set by hand, by class index (0 is the lowest class); a class without one
        /// shows its range.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        class_labels: BTreeMap<usize, String>,
    },
    /// Marker size grows with the value.
    Proportional {
        field: String,
        base: Symbol,
        min_size: f32,
        max_size: f32,
        min: f64,
        max: f64,
    },
}

/// The legend label of a Unique Values renderer's fallback class.
pub const OTHER_VALUES: &str = "<all other values>";

/// One legend row.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub label: String,
    pub symbol: Symbol,
    /// The Unique Values fallback row (`<all other values>`), told apart from a class whatever
    /// its label: a legend can leave it out without touching a class that bears the same name.
    pub fallback: bool,
}

/// Most unique-value classes created automatically.
pub const MAX_UNIQUE: usize = 64;

/// The colour of a new Unique Values renderer's `<all other values>` class.
pub const OTHER_FILL: Rgba = Rgba::rgb(0xB0, 0xB0, 0xB0);

/// The longest legend label set by hand, in characters.
pub const MAX_LABEL: usize = 200;

/// One class of a Unique Values or Graduated Colors renderer, as a command names it.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassRef {
    /// A Unique Values class, by its value.
    Value(String),
    /// A Graduated Colors class, by index (0 is the lowest).
    Index(usize),
    /// The Unique Values fallback, `<all other values>`.
    Other,
}

/// What [`Renderer::set_class_style`] changes in a class; `None` leaves that part as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassStyle {
    pub color: Option<Rgba>,
    pub outline: Option<Rgba>,
    /// `Some("")` clears a label set by hand, so the class shows its value or range again.
    pub label: Option<String>,
}

impl Renderer {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Renderer::Single(_) => "Single Symbol",
            Renderer::Unique { .. } => "Unique Values",
            Renderer::Graduated { .. } => "Graduated Colors",
            Renderer::Proportional { .. } => "Proportional Symbols",
        }
    }

    pub fn field(&self) -> Option<&str> {
        match self {
            Renderer::Single(_) => None,
            Renderer::Unique { field, .. } | Renderer::Graduated { field, .. } | Renderer::Proportional { field, .. } => Some(field),
        }
    }

    /// The first symbol (the single symbol, or a representative).
    pub fn base_symbol(&self) -> Symbol {
        match self {
            Renderer::Single(s) => *s,
            Renderer::Unique { other, classes, .. } => classes.first().map(|c| c.1).unwrap_or(*other),
            Renderer::Graduated { symbols, .. } => symbols.last().copied().unwrap_or(Symbol::default_for(GeometryType::Polygon, 0)),
            Renderer::Proportional { base, .. } => *base,
        }
    }

    /// Builds a unique-values renderer on `field`, from the rows `rows` keeps (a row past its end
    /// counts, so `&[]` keeps them all).
    pub fn unique(ds: &Dataset, rows: &[bool], field: &str, ramp: Ramp, base: Symbol) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        let vals = ds.unique_values(fi, rows, MAX_UNIQUE);
        let n = vals.len();
        let classes = vals.into_iter().enumerate().map(|(k, v)| (v, base.with_fill(ramp.pick(k, n), ds.geometry_type))).collect();
        let other = base.with_fill(OTHER_FILL, ds.geometry_type);
        Ok(Renderer::Unique {
            field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(),
            classes,
            other,
            ramp,
            class_labels: BTreeMap::new(),
            other_label: None,
        })
    }

    /// Builds a graduated-colours renderer on a numeric `field`, classing the rows `rows` keeps (a
    /// row past its end counts, so `&[]` keeps them all).
    pub fn graduated(
        ds: &Dataset,
        rows: &[bool],
        field: &str,
        classes: usize,
        method: ClassMethod,
        ramp: Ramp,
        base: Symbol,
    ) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        if !ds.fields.get(fi).is_some_and(|f| f.ty.is_numeric()) {
            return Err(format!("'{field}' is not a numeric field"));
        }
        let mut vals: Vec<f64> = ds.kept_features(rows).filter_map(|f| f.attrs.get(fi).and_then(|v| v.as_f64())).collect();
        if vals.is_empty() {
            return Err(format!("'{field}' has no numeric values"));
        }
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let k = classes.clamp(1, 12);
        let breaks = class_breaks(&vals, k, method);
        let n = breaks.len();
        let symbols = (0..n).map(|i| base.with_fill(ramp.pick(i, n), ds.geometry_type)).collect();
        let min = vals.first().copied().unwrap_or(0.0);
        Ok(Renderer::Graduated {
            field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(),
            breaks,
            symbols,
            ramp,
            method,
            min,
            class_labels: BTreeMap::new(),
        })
    }

    /// Builds a proportional-symbols renderer on a numeric `field`, sized over the rows `rows` keeps
    /// (a row past its end counts, so `&[]` keeps them all).
    pub fn proportional(ds: &Dataset, rows: &[bool], field: &str, base: Symbol) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        if !ds.fields.get(fi).is_some_and(|f| f.ty.is_numeric()) {
            return Err(format!("'{field}' is not a numeric field"));
        }
        let (min, max) = ds.numeric_range(fi, rows).ok_or_else(|| format!("'{field}' has no numeric values"))?;
        let mut base = base;
        if base.size == 0.0 {
            base.size = 6.0;
        }
        Ok(Renderer::Proportional {
            field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(),
            base,
            min_size: 4.0,
            max_size: 28.0,
            min,
            max,
        })
    }

    /// Symbol for a feature row.
    pub fn symbol_for(&self, ds: &Dataset, row: usize) -> Symbol {
        match self {
            Renderer::Single(s) => *s,
            Renderer::Unique { field, classes, other, .. } => {
                let Some(fi) = ds.field_index(field) else { return *other };
                // Text compares as it is; other values as they print.
                let printed;
                let v = match ds.value(row, fi) {
                    rustgis_data::Value::Text(t) => t.as_str(),
                    v => {
                        printed = v.to_string();
                        printed.as_str()
                    }
                };
                classes.iter().find(|(k, _)| k == v).map(|c| c.1).unwrap_or(*other)
            }
            Renderer::Graduated { field, breaks, symbols, .. } => {
                let fallback = symbols.first().copied().unwrap_or(Symbol::default_for(ds.geometry_type, 0));
                let Some(v) = ds.field_index(field).and_then(|fi| ds.value(row, fi).as_f64()) else {
                    return Symbol { fill: Rgba::new(0xC8, 0xC8, 0xC8, 140), ..fallback };
                };
                let i = breaks.iter().position(|b| v <= *b).unwrap_or(breaks.len().saturating_sub(1));
                symbols.get(i).copied().unwrap_or(fallback)
            }
            Renderer::Proportional { field, base, min_size, max_size, min, max } => {
                let Some(v) = ds.field_index(field).and_then(|fi| ds.value(row, fi).as_f64()) else { return *base };
                let t = if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.5 };
                // Area-proportional (Flannery-free square root scaling).
                Symbol { size: min_size + (max_size - min_size) * t.sqrt() as f32, ..*base }
            }
        }
    }

    /// The legend for the features `rows` keeps (a row past its end counts, so `&[]` keeps them
    /// all): a Unique Values class only when some kept feature has it, and `<all other values>`
    /// only when some kept feature matches no class and its symbol is not fully transparent (#207).
    /// The classes may come from a style file or from before a definition query changed, so they
    /// can name values no kept feature has. The other renderers list their classes as they are.
    pub fn legend_for(&self, ds: &Dataset, rows: &[bool]) -> Vec<LegendEntry> {
        let Renderer::Unique { field, classes, other, class_labels, other_label, .. } = self else { return self.legend() };
        // A value matches its first class, as in `symbol_for`.
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, (k, _)) in classes.iter().enumerate() {
            index.entry(k.as_str()).or_insert(i);
        }
        let fi = ds.field_index(field);
        let mut used = vec![false; classes.len()];
        let mut other_used = false;
        // Distinct classes still unseen: once every class and "other" are in, the rest can't add any.
        let mut unseen = index.len() + 1;
        // One buffer for every value's text, rather than a string per feature.
        let mut text = String::new();
        for r in (0..ds.features.len()).filter(|r| rows.get(*r).copied().unwrap_or(true)) {
            let class = fi.and_then(|fi| {
                use std::fmt::Write;
                text.clear();
                write!(text, "{}", ds.value(r, fi)).ok()?;
                index.get(text.as_str()).copied()
            });
            let seen = match class.and_then(|i| used.get_mut(i)) {
                Some(u) => std::mem::replace(u, true),
                None => std::mem::replace(&mut other_used, true),
            };
            if !seen {
                unseen -= 1;
                if unseen == 0 {
                    break;
                }
            }
        }
        let mut v: Vec<LegendEntry> = classes
            .iter()
            .zip(&used)
            .filter(|(_, u)| **u)
            .map(|((k, s), _)| LegendEntry { label: unique_label(class_labels, k), symbol: *s, fallback: false })
            .collect();
        v.sort_by_cached_key(|e| e.label.to_lowercase());
        // A fully transparent fallback draws nothing, so it has nothing to stand for in a legend.
        // Lines draw with their stroke only.
        let visible = other.stroke.a() > 0 || (ds.geometry_type != GeometryType::Polyline && other.fill.a() > 0);
        if other_used && visible {
            v.push(LegendEntry { label: other_label.clone().unwrap_or_else(|| OTHER_VALUES.into()), symbol: *other, fallback: true });
        }
        v
    }

    /// The order `legend()` lists the Unique Values classes in: their indices in `classes`,
    /// sorted by label without regard to case (a stable sort, so a repeated value keeps its first
    /// class first). Empty for the other renderers, whose legend follows their classes.
    pub fn class_order(&self) -> Vec<usize> {
        let Renderer::Unique { classes, class_labels, .. } = self else { return Vec::new() };
        let mut order: Vec<usize> = (0..classes.len()).collect();
        order.sort_by_cached_key(|i| classes.get(*i).map(|(k, _)| unique_label(class_labels, k).to_lowercase()));
        order
    }

    pub fn legend(&self) -> Vec<LegendEntry> {
        match self {
            Renderer::Single(_) => Vec::new(),
            Renderer::Unique { classes, other, class_labels, other_label, .. } => {
                // Keep symbols attached to their sorted labels and the fallback at the end.
                let mut v: Vec<LegendEntry> = self
                    .class_order()
                    .into_iter()
                    .filter_map(|i| classes.get(i))
                    .map(|(k, s)| LegendEntry { label: unique_label(class_labels, k), symbol: *s, fallback: false })
                    .collect();
                v.push(LegendEntry { label: other_label.clone().unwrap_or_else(|| OTHER_VALUES.into()), symbol: *other, fallback: true });
                v
            }
            Renderer::Graduated { breaks, symbols, min, class_labels, .. } => {
                let mut lo = *min;
                breaks
                    .iter()
                    .zip(symbols)
                    .enumerate()
                    .map(|(i, (b, s))| {
                        let label = class_labels.get(&i).cloned().unwrap_or_else(|| range_label(lo, *b));
                        lo = *b;
                        LegendEntry { label, symbol: *s, fallback: false }
                    })
                    .collect()
            }
            Renderer::Proportional { base, min_size, max_size, min, max, .. } => vec![
                LegendEntry { label: fmt_num(*min), symbol: Symbol { size: *min_size, ..*base }, fallback: false },
                LegendEntry {
                    label: fmt_num((min + max) / 2.0),
                    symbol: Symbol { size: min_size + (max_size - min_size) * 0.707, ..*base },
                    fallback: false,
                },
                LegendEntry { label: fmt_num(*max), symbol: Symbol { size: *max_size, ..*base }, fallback: false },
            ],
        }
    }

    /// Recolours every class from a ramp.
    pub fn apply_ramp(&mut self, ramp: Ramp, g: GeometryType) {
        match self {
            Renderer::Unique { classes, ramp: r, .. } => {
                let n = classes.len();
                for (k, c) in classes.iter_mut().enumerate() {
                    c.1 = c.1.with_fill(ramp.pick(k, n), g);
                }
                *r = ramp;
            }
            Renderer::Graduated { symbols, ramp: r, .. } => {
                let n = symbols.len();
                for (k, s) in symbols.iter_mut().enumerate() {
                    *s = s.with_fill(ramp.pick(k, n), g);
                }
                *r = ramp;
            }
            _ => {}
        }
    }

    /// Sets one class's colour, outline colour and legend label (#314). The colour goes through
    /// [`Symbol::with_fill`], as a ramp's does, so a line class takes it as its line colour and a
    /// polygon class gets a matching outline unless `outline` names one; a polygon class also keeps
    /// its own transparency, so the colour's alpha applies to points and lines only. Returns the class's
    /// legend row as it now is.
    pub fn set_class_style(&mut self, class: &ClassRef, style: &ClassStyle, g: GeometryType) -> Result<LegendEntry, String> {
        let label = style.label.as_deref().map(normalize_label);
        let restyle = |s: &mut Symbol| {
            if let Some(c) = style.color {
                *s = s.with_fill(c, g);
            }
            if let Some(c) = style.outline {
                s.stroke = c;
            }
        };
        // The class's row in `legend()`, which lists the classes and then `other`. Unique Values
        // rows are sorted by label, so that class's row is built here rather than looked up.
        let row = match (&mut *self, class) {
            (Renderer::Unique { classes, class_labels, .. }, ClassRef::Value(v)) => {
                let i = classes.iter().position(|(k, _)| k == v).ok_or_else(|| format!("no class with the value '{v}'"))?;
                if let Some((k, s)) = classes.get_mut(i) {
                    restyle(s);
                    match label {
                        // Empty, or the value itself: the class shows its value, with no label of its own.
                        Some(t) if t.is_empty() || t == *k => {
                            class_labels.remove(k.as_str());
                        }
                        Some(t) => {
                            class_labels.insert(k.clone(), t);
                        }
                        None => {}
                    }
                }
                return classes
                    .get(i)
                    .map(|(k, s)| LegendEntry { label: unique_label(class_labels, k), symbol: *s, fallback: false })
                    .ok_or_else(|| "the class has no legend row".into());
            }
            (Renderer::Unique { classes, other, other_label, .. }, ClassRef::Other) => {
                restyle(other);
                if let Some(t) = label {
                    *other_label = (!t.is_empty() && t != OTHER_VALUES).then_some(t);
                }
                classes.len()
            }
            (Renderer::Graduated { symbols, class_labels, breaks, min, .. }, ClassRef::Index(i)) => {
                // The range the class shows when it has no label of its own.
                let lo = if *i == 0 { Some(*min) } else { breaks.get(*i - 1).copied() };
                let range = lo.zip(breaks.get(*i).copied()).map(|(lo, hi)| range_label(lo, hi));
                let n = symbols.len();
                let s = symbols.get_mut(*i).ok_or_else(|| format!("no class {i}: this renderer has {n} (0 to {})", n.saturating_sub(1)))?;
                restyle(s);
                match label {
                    Some(t) if t.is_empty() || Some(&t) == range.as_ref() => {
                        class_labels.remove(i);
                    }
                    Some(t) => {
                        class_labels.insert(*i, t);
                    }
                    None => {}
                }
                *i
            }
            (Renderer::Unique { .. }, ClassRef::Index(_)) => {
                return Err("Unique Values classes are named by their value, not by number".into());
            }
            (Renderer::Graduated { .. }, _) => return Err("Graduated Colors classes are numbered from 0, the lowest".into()),
            _ => return Err("only Unique Values and Graduated Colors have classes to style".into()),
        };
        self.legend().into_iter().nth(row).ok_or_else(|| "the class has no legend row".into())
    }

    /// Carries hand-made class edits over from `old` when the symbology is applied again (#314).
    /// On the same field, a class that is still there keeps its legend label (a Graduated Colors
    /// class keeps its edits only while the ranges are the same: they belong to a range). With the
    /// same colour scheme too, it keeps the colour and outline set by hand: those that aren't what
    /// the scheme gave it at its place in `old` (see [`carry_class`]). The other classes take the
    /// new scheme's colours at their new places, on the new base symbol. A different scheme
    /// recolours every class (picking a scheme is asking for its colours); a different field
    /// starts fresh.
    pub fn carry_edits_from(&mut self, old: &Renderer, g: GeometryType) {
        match (self, old) {
            (
                Renderer::Unique { field, classes, other, ramp, class_labels, other_label },
                Renderer::Unique { field: of, classes: oc, other: oo, ramp: or, class_labels: ol, other_label: ool },
            ) if field == of => {
                let same_scheme = ramp == or;
                let n = oc.len();
                for (k, s) in classes.iter_mut() {
                    if let Some(t) = ol.get(k.as_str()) {
                        class_labels.insert(k.clone(), t.clone());
                    }
                    if same_scheme
                        && let Some(j) = oc.iter().position(|(ok, _)| ok == k)
                        && let Some((_, os)) = oc.get(j)
                    {
                        *s = carry_class(*s, os, or.pick(j, n), g);
                    }
                }
                other_label.clone_from(ool);
                if same_scheme {
                    *other = carry_class(*other, oo, OTHER_FILL, g);
                }
            }
            (
                Renderer::Graduated { field, breaks, symbols, ramp, min, class_labels, .. },
                Renderer::Graduated { field: of, breaks: ob, symbols: os, ramp: or, min: om, class_labels: ol, .. },
            ) if field == of => {
                // A Graduated Colors edit belongs to a range: it stays only while the ranges do.
                if breaks != ob || min != om {
                    return;
                }
                class_labels.clone_from(ol);
                if ramp == or && symbols.len() == os.len() {
                    let n = os.len();
                    for (i, (s, o)) in symbols.iter_mut().zip(os).enumerate() {
                        *s = carry_class(*s, o, or.pick(i, n), g);
                    }
                }
            }
            _ => {}
        }
    }

    /// Applies `f` to every symbol.
    pub fn map_symbols(&mut self, f: impl Fn(&mut Symbol)) {
        match self {
            Renderer::Single(s) => f(s),
            Renderer::Unique { classes, other, .. } => {
                classes.iter_mut().for_each(|c| f(&mut c.1));
                f(other);
            }
            Renderer::Graduated { symbols, .. } => symbols.iter_mut().for_each(f),
            Renderer::Proportional { base, .. } => f(base),
        }
    }
}

/// `new` with the hand-made parts of `old`, a class whose scheme colour was `scheme`: the fill
/// when it isn't that colour, and the outline when it isn't the one [`Symbol::with_fill`] derives
/// (a polygon's shaded fill, a line's own colour; for points, the new base symbol's outline).
/// A polygon's fill compares without alpha (`with_fill` keeps a polygon's own transparency) and,
/// when edited, carries over whole, transparency included; a point's or a line's compares with it,
/// since `with_fill` gives them the colour's alpha as it is. Outlines compare whole.
fn carry_class(new: Symbol, old: &Symbol, scheme: Rgba, g: GeometryType) -> Symbol {
    let rgb = |c: Rgba| (c.r(), c.g(), c.b());
    let mut s = new;
    let edited = if g == GeometryType::Polygon { rgb(old.fill) != rgb(scheme) } else { old.fill != scheme };
    if edited {
        let c = if g == GeometryType::Polygon { Rgba::rgb(old.fill.r(), old.fill.g(), old.fill.b()) } else { old.fill };
        s = s.with_fill(c, g);
        if g == GeometryType::Polygon {
            s.fill = old.fill;
        }
    }
    let derived = match g {
        GeometryType::Polygon => Rgba::rgb(old.fill.r(), old.fill.g(), old.fill.b()).shade(0.6),
        GeometryType::Polyline => old.fill,
        _ => new.stroke,
    };
    if old.stroke != derived {
        s.stroke = old.stroke;
    }
    s
}

/// A legend label as [`Renderer::set_class_style`] keeps it: one line (control characters such as
/// line breaks become spaces), trimmed, cut to [`MAX_LABEL`] characters and trimmed again (a cut
/// can end in a space). An editor compares its text this way to tell whether it changed.
pub fn normalize_label(t: &str) -> String {
    let t: String = t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    t.trim().chars().take(MAX_LABEL).collect::<String>().trim_end().to_string()
}

/// A Unique Values class's legend label: the one set by hand, or its value.
fn unique_label(class_labels: &BTreeMap<String, String>, value: &str) -> String {
    class_labels.get(value).cloned().unwrap_or_else(|| value.to_string())
}

/// A Graduated Colors class's legend label when it has none of its own: its range.
fn range_label(lo: f64, hi: f64) -> String {
    format!("{} – {}", fmt_num(lo), fmt_num(hi))
}

/// Compact number formatting for legends: 1.2M, 35K, 0.42.
pub fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}K", v / 1e3)
    } else if a >= 100.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Upper class bounds for sorted values.
pub fn class_breaks(sorted: &[f64], k: usize, method: ClassMethod) -> Vec<f64> {
    let n = sorted.len();
    let (Some(&lo), Some(&hi)) = (sorted.first(), sorted.last()) else { return Vec::new() };
    if k <= 1 || hi <= lo {
        return vec![hi];
    }
    let mut b = match method {
        ClassMethod::EqualInterval => (1..=k).map(|i| lo + (hi - lo) * i as f64 / k as f64).collect::<Vec<_>>(),
        ClassMethod::Quantile => (1..=k).map(|i| sorted.get(((n * i) / k).saturating_sub(1).min(n - 1)).copied().unwrap_or(hi)).collect(),
        ClassMethod::NaturalBreaks => jenks(sorted, k),
    };
    b.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
    if let Some(l) = b.last_mut() {
        *l = hi;
    }
    b
}

/// Jenks natural breaks (Fisher's dynamic programming), on a sample of at most 1000 values.
fn jenks(sorted: &[f64], k: usize) -> Vec<f64> {
    let data: Vec<f64> = if sorted.len() > 1000 {
        (0..1000).filter_map(|i| sorted.get(i * sorted.len() / 1000)).copied().collect()
    } else {
        sorted.to_vec()
    };
    let n = data.len();
    let k = k.min(n);
    if k < 2 {
        return vec![data.last().copied().unwrap_or(0.0)];
    }
    // lower[i][j]: first index of class j for data[0..=i]; var[i][j]: within-class variance.
    let mut lower = vec![vec![0usize; k + 1]; n + 1];
    let mut var = vec![vec![f64::INFINITY; k + 1]; n + 1];
    for j in 1..=k {
        lower[1][j] = 1;
        var[1][j] = 0.0;
    }
    for l in 2..=n {
        let (mut s1, mut s2, mut w) = (0.0, 0.0, 0.0);
        let mut v = 0.0;
        for m in 1..=l {
            let i3 = l - m + 1;
            let val = data[i3 - 1];
            s2 += val * val;
            s1 += val;
            w += 1.0;
            v = s2 - (s1 * s1) / w;
            let i4 = i3 - 1;
            if i4 != 0 {
                for j in 2..=k {
                    if var[l][j] >= v + var[i4][j - 1] {
                        lower[l][j] = i3;
                        var[l][j] = v + var[i4][j - 1];
                    }
                }
            }
        }
        lower[l][1] = 1;
        var[l][1] = v;
    }
    let mut breaks = vec![0.0; k];
    breaks[k - 1] = data[n - 1];
    let mut kk = n;
    for j in (2..=k).rev() {
        let id = lower[kk][j].saturating_sub(2);
        breaks[j - 2] = data[id];
        kk = lower[kk][j].saturating_sub(1).max(1);
    }
    breaks
}

/// Label settings for a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Labeling {
    pub enabled: bool,
    pub field: String,
    pub size: f32,
    pub color: Rgba,
    pub halo: bool,
    pub bold: bool,
    /// Labels draw only when zoomed in beyond 1:`min_scale`, if set.
    #[serde(default)]
    pub min_scale: Option<f64>,
    /// Labels draw only when zoomed out beyond 1:`max_scale`, if set.
    #[serde(default)]
    pub max_scale: Option<f64>,
    /// Labels draw only when the viewer is at least this far away (metres), if set.
    #[serde(default)]
    pub closest: Option<f64>,
    /// Labels draw only when the viewer is at most this far away (metres), if set.
    #[serde(default)]
    pub farthest: Option<f64>,
    /// Where a label sits relative to its feature.
    #[serde(default)]
    pub placement: LabelPlacement,
}

/// Where labels sit relative to their feature's label point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelPlacement {
    /// Above points, centred on lines and polygons.
    #[default]
    Auto,
    Center,
    Above,
    Below,
    Left,
    Right,
    AboveRight,
    AboveLeft,
    BelowRight,
    BelowLeft,
}

impl LabelPlacement {
    pub const ALL: [LabelPlacement; 10] = [
        LabelPlacement::Auto,
        LabelPlacement::Center,
        LabelPlacement::Above,
        LabelPlacement::Below,
        LabelPlacement::Left,
        LabelPlacement::Right,
        LabelPlacement::AboveRight,
        LabelPlacement::AboveLeft,
        LabelPlacement::BelowRight,
        LabelPlacement::BelowLeft,
    ];
    pub fn code(self) -> &'static str {
        match self {
            LabelPlacement::Auto => "auto",
            LabelPlacement::Center => "center",
            LabelPlacement::Above => "above",
            LabelPlacement::Below => "below",
            LabelPlacement::Left => "left",
            LabelPlacement::Right => "right",
            LabelPlacement::AboveRight => "aboveRight",
            LabelPlacement::AboveLeft => "aboveLeft",
            LabelPlacement::BelowRight => "belowRight",
            LabelPlacement::BelowLeft => "belowLeft",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LabelPlacement::Auto => "Automatic",
            LabelPlacement::Center => "Centered",
            LabelPlacement::Above => "Above",
            LabelPlacement::Below => "Below",
            LabelPlacement::Left => "Left",
            LabelPlacement::Right => "Right",
            LabelPlacement::AboveRight => "Above Right",
            LabelPlacement::AboveLeft => "Above Left",
            LabelPlacement::BelowRight => "Below Right",
            LabelPlacement::BelowLeft => "Below Left",
        }
    }
    pub fn from_code(s: &str) -> Option<LabelPlacement> {
        LabelPlacement::ALL.into_iter().find(|p| p.code().eq_ignore_ascii_case(s.trim()))
    }
    /// The label centre's offset from the label point, in points, for a label `w` wide and `h`
    /// tall next to a marker of size `marker` (0 for lines and polygons).
    pub fn offset(self, w: f32, h: f32, marker: f32, is_point: bool) -> (f32, f32) {
        let gap = marker / 2.0 + 2.0;
        let (dx, dy) = (w / 2.0 + gap, h / 2.0 + gap);
        match self {
            LabelPlacement::Auto if is_point => (0.0, -(marker / 2.0 + h * 0.6)),
            LabelPlacement::Auto | LabelPlacement::Center => (0.0, 0.0),
            LabelPlacement::Above => (0.0, -dy),
            LabelPlacement::Below => (0.0, dy),
            LabelPlacement::Left => (-dx, 0.0),
            LabelPlacement::Right => (dx, 0.0),
            LabelPlacement::AboveRight => (dx, -dy),
            LabelPlacement::AboveLeft => (-dx, -dy),
            LabelPlacement::BelowRight => (dx, dy),
            LabelPlacement::BelowLeft => (-dx, dy),
        }
    }
}

impl Labeling {
    /// Whether labels draw at a scale denominator and viewing distance (metres).
    pub fn in_range(&self, scale: f64, distance: f64) -> bool {
        self.min_scale.is_none_or(|m| scale <= m)
            && self.max_scale.is_none_or(|m| scale >= m)
            && self.closest.is_none_or(|d| distance >= d)
            && self.farthest.is_none_or(|d| distance <= d)
    }

    /// Off, on the first text field that looks like a name.
    pub fn default_for(ds: &Dataset) -> Labeling {
        let field = ds
            .fields
            .iter()
            .find(|f| f.ty == FieldType::Text && f.name.to_ascii_lowercase().contains("name"))
            .or_else(|| ds.fields.iter().find(|f| f.ty == FieldType::Text))
            .or_else(|| ds.fields.first())
            .map(|f| f.name.clone())
            .unwrap_or_default();
        Labeling {
            enabled: false,
            field,
            size: 10.0,
            color: Rgba::rgb(0x2B, 0x2B, 0x2B),
            halo: true,
            bold: false,
            min_scale: None,
            max_scale: None,
            closest: None,
            farthest: None,
            placement: LabelPlacement::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, Value};

    #[test]
    fn the_legend_lists_only_classes_kept_features_have() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("g", FieldType::Text)]);
        for g in ["a", "b", "a", "z"] {
            d.push(None, vec![Value::Text(g.into())]);
        }
        let s = |c| Symbol::default_for(GeometryType::Polygon, c);
        // "c" is no feature's value; "z" matches no class.
        let r = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), ("b".into(), s(1)), ("c".into(), s(2))],
            other: s(3),
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        let labels = |rows: &[bool]| r.legend_for(&d, rows).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(&[]), ["a", "b", "<all other values>"]);
        assert_eq!(labels(&[true, false, true, false]), ["a"], "<all other values> only when used");
        assert_eq!(labels(&[false, true]), ["a", "b", "<all other values>"], "rows past the end (a, z) count as kept");
        assert_eq!(labels(&[false, false, false, true]), ["<all other values>"]);
        assert!(labels(&[false; 4]).is_empty());
        // The full legend is unchanged.
        assert_eq!(r.legend().len(), 4);
        // A class listed twice: the first matches, and the scan still stops once all are seen.
        let twice = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), ("a".into(), s(1)), ("b".into(), s(2))],
            other: s(3),
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        let shown = twice.legend_for(&d, &[]);
        assert_eq!(shown.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["a", "b", "<all other values>"]);
        assert_eq!(shown[0].symbol, s(0));
        // A field the data no longer has: every feature is "other".
        let gone = Renderer::Unique {
            field: "nope".into(),
            classes: vec![("a".into(), s(0))],
            other: s(3),
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        assert_eq!(gone.legend_for(&d, &[]).len(), 1);
        // The fallback row is marked as such, whatever a class is called.
        let named = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), (OTHER_VALUES.into(), s(1))],
            other: s(3),
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        let mut d2 = d.clone();
        d2.push(None, vec![Value::Text(OTHER_VALUES.into())]);
        let rows = named.legend_for(&d2, &[]);
        assert_eq!(
            rows.iter().map(|e| (e.label.as_str(), e.fallback)).collect::<Vec<_>>(),
            [(OTHER_VALUES, false), ("a", false), (OTHER_VALUES, true)]
        );
        // Other renderers list their classes as they are.
        assert_eq!(Renderer::Single(s(0)).legend_for(&d, &[false; 4]), Vec::new());
    }

    #[test]
    fn a_transparent_fallback_is_left_out_of_the_legend() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("g", FieldType::Text)]);
        for g in ["a", "z"] {
            d.push(None, vec![Value::Text(g.into())]);
        }
        let s = |c| Symbol::default_for(GeometryType::Polygon, c);
        let mut other = s(3);
        other.fill = other.fill.with_alpha(0);
        other.stroke = other.stroke.with_alpha(0);
        let r = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0))],
            other,
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        let labels = |r: &Renderer, rows: &[bool]| r.legend_for(&d, rows).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(&r, &[]), ["a"], "z falls in the invisible fallback");
        assert!(labels(&r, &[false, true]).is_empty(), "only the invisible fallback is used");
        // A visible stroke alone still stands for the features.
        let mut stroked = other;
        stroked.stroke = stroked.stroke.with_alpha(255);
        let r2 = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0))],
            other: stroked,
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        assert_eq!(labels(&r2, &[]), ["a", OTHER_VALUES]);
        // A line's fill draws nothing: a fallback with only a fill is invisible on lines.
        let mut lines = Dataset::new("l", GeometryType::Polyline, vec![Field::new("g", FieldType::Text)]);
        lines.push(None, vec![Value::Text("z".into())]);
        let mut filled = other;
        filled.fill = filled.fill.with_alpha(255);
        let r3 = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0))],
            other: filled,
            ramp: Ramp::Ember,
            class_labels: Default::default(),
            other_label: None,
        };
        assert!(r3.legend_for(&lines, &[]).is_empty());
        assert_eq!(labels(&r3, &[]), ["a", OTHER_VALUES], "a polygon fill still shows");
        // The full legend (the symbology editor's list) keeps it.
        assert_eq!(r.legend().len(), 2);
    }

    #[test]
    fn categorical_children_sort_case_insensitively_without_changing_symbols() {
        let names = ["Zulu", "alpha", "Bravo", "ALPHA", "éclair", "École"];
        let mut data = Dataset::new("categories", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        let classes: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                data.push(None, vec![Value::Text((*name).into())]);
                ((*name).into(), Symbol::default_for(GeometryType::Point, i as u32))
            })
            .collect();
        let renderer = Renderer::Unique {
            field: "name".into(),
            classes: classes.clone(),
            other: Symbol::default_for(GeometryType::Point, 99),
            ramp: Ramp::Ember,
            class_labels: BTreeMap::new(),
            other_label: None,
        };
        let legend = renderer.legend();
        assert_eq!(
            legend.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["alpha", "ALPHA", "Bravo", "Zulu", "éclair", "École", "<all other values>"]
        );
        for (i, (name, symbol)) in classes.iter().enumerate() {
            assert_eq!(legend.iter().find(|e| &e.label == name).unwrap().symbol, *symbol);
            assert_eq!(renderer.symbol_for(&data, i), *symbol);
        }
        let filtered = renderer.legend_for(&data, &[true, false, true, true, false, false]);
        assert_eq!(filtered.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["ALPHA", "Bravo", "Zulu"]);
        for entry in &filtered {
            assert_eq!(entry.symbol, classes.iter().find(|(name, _)| name == &entry.label).unwrap().1);
            assert!(!entry.fallback);
        }
        data.push(None, vec![Value::Text("unclassified".into())]);
        assert_eq!(renderer.legend_for(&data, &[]), legend);
    }

    #[test]
    fn breaks() {
        let v: Vec<f64> = vec![1.0, 2.0, 3.0, 10.0, 11.0, 12.0, 50.0, 51.0, 52.0];
        let j = class_breaks(&v, 3, ClassMethod::NaturalBreaks);
        assert_eq!(j, vec![3.0, 12.0, 52.0]);
        assert_eq!(class_breaks(&v, 3, ClassMethod::EqualInterval).len(), 3);
        assert_eq!(class_breaks(&[], 3, ClassMethod::Quantile), Vec::<f64>::new());
        assert_eq!(class_breaks(&[5.0, 5.0], 4, ClassMethod::NaturalBreaks), vec![5.0]);
    }

    #[test]
    fn graduated_symbols_by_value() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("v", FieldType::Double)]);
        for v in [1.0, 2.0, 50.0, 60.0] {
            d.push(None, vec![Value::Float(v)]);
        }
        d.push(None, vec![Value::Null]);
        let r =
            Renderer::graduated(&d, &[], "v", 2, ClassMethod::NaturalBreaks, Ramp::Ember, Symbol::default_for(GeometryType::Polygon, 0))
                .unwrap();
        assert_ne!(r.symbol_for(&d, 0).fill, r.symbol_for(&d, 3).fill);
        assert_eq!(r.legend().len(), 2);
        let _ = r.symbol_for(&d, 4);
        let _ = r.symbol_for(&d, 999);
        assert!(
            Renderer::graduated(&d, &[], "nope", 2, ClassMethod::Quantile, Ramp::Ember, Symbol::default_for(GeometryType::Polygon, 0))
                .is_err()
        );
    }

    #[test]
    fn classes_come_from_the_kept_rows() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("v", FieldType::Double)]);
        for v in [1.0, 2.0, 3.0, 1000.0] {
            d.push(None, vec![Value::Float(v)]);
        }
        let base = Symbol::default_for(GeometryType::Polygon, 0);
        let rows = [true, true, true, false];
        let Ok(Renderer::Graduated { breaks, min, .. }) =
            Renderer::graduated(&d, &rows, "v", 2, ClassMethod::EqualInterval, Ramp::Ember, base)
        else {
            panic!("graduated renderer expected")
        };
        assert_eq!(breaks.last().copied(), Some(3.0));
        assert_eq!(min, 1.0);
        // A short row list keeps the rows past its end.
        let Ok(Renderer::Graduated { breaks, .. }) =
            Renderer::graduated(&d, &[false], "v", 2, ClassMethod::EqualInterval, Ramp::Ember, base)
        else {
            panic!("graduated renderer expected")
        };
        assert_eq!(breaks.last().copied(), Some(1000.0));
        assert!(Renderer::graduated(&d, &[false; 4], "v", 2, ClassMethod::Quantile, Ramp::Ember, base).is_err());
        let Ok(Renderer::Unique { classes, .. }) = Renderer::unique(&d, &[false, true, false, true], "v", Ramp::Harbour, base) else {
            panic!("unique renderer expected")
        };
        let keys: Vec<&str> = classes.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["1000", "2"]);
    }

    fn unique(classes: &[(&str, Symbol)], other: Symbol, ramp: Ramp) -> Renderer {
        Renderer::Unique {
            field: "g".into(),
            classes: classes.iter().map(|(k, s)| (k.to_string(), *s)).collect(),
            other,
            ramp,
            class_labels: Default::default(),
            other_label: None,
        }
    }

    fn graduated(breaks: &[f64], symbols: &[Symbol], ramp: Ramp) -> Renderer {
        Renderer::Graduated {
            field: "v".into(),
            breaks: breaks.to_vec(),
            symbols: symbols.to_vec(),
            ramp,
            method: ClassMethod::Quantile,
            min: 0.0,
            class_labels: Default::default(),
        }
    }

    fn labels_of(r: &Renderer) -> Vec<String> {
        r.legend().into_iter().map(|e| e.label).collect()
    }

    #[test]
    fn a_class_takes_its_own_colour_outline_and_label() {
        let g = GeometryType::Polygon;
        let s = |c| Symbol::default_for(g, c);
        let (red, blue) = (Rgba::rgb(0xC0, 0x30, 0x20), Rgba::rgb(0, 0, 0xFF));
        let mut r = unique(&[("a", s(0)), ("b", s(1))], s(3), Ramp::Ember);
        let style = |color, outline, label: Option<&str>| ClassStyle { color, outline, label: label.map(str::to_string) };
        // The colour goes through with_fill, as a ramp's does; the label is trimmed.
        let e = r.set_class_style(&ClassRef::Value("b".into()), &style(Some(red), None, Some("  Bee ")), g).unwrap();
        assert_eq!((e.label.as_str(), e.symbol, e.fallback), ("Bee", s(1).with_fill(red, g), false));
        assert_eq!(labels_of(&r), ["a", "Bee", OTHER_VALUES]);
        // The fallback class, with an outline alone: the fill stays.
        let e = r.set_class_style(&ClassRef::Other, &style(None, Some(blue), Some("Others")), g).unwrap();
        assert!(e.fallback);
        assert_eq!((e.symbol.stroke, e.symbol.fill), (blue, s(3).fill));
        // The legend of what is drawn shows the labels too.
        let mut d = Dataset::new("t", g, vec![Field::new("g", FieldType::Text)]);
        for v in ["b", "z"] {
            d.push(None, vec![Value::Text(v.into())]);
        }
        assert_eq!(r.legend_for(&d, &[]).into_iter().map(|e| e.label).collect::<Vec<_>>(), ["Bee", "Others"]);
        // A label that is the value itself (or <all other values>) is no label of its own.
        r.set_class_style(&ClassRef::Value("b".into()), &style(None, None, Some("b")), g).unwrap();
        assert!(matches!(&r, Renderer::Unique { class_labels, .. } if class_labels.is_empty()));
        r.set_class_style(&ClassRef::Value("b".into()), &style(None, None, Some("Bee")), g).unwrap();
        r.set_class_style(&ClassRef::Other, &style(None, None, Some(OTHER_VALUES)), g).unwrap();
        assert!(matches!(&r, Renderer::Unique { other_label: None, .. }));
        r.set_class_style(&ClassRef::Other, &style(None, None, Some("Others")), g).unwrap();
        // An empty (or blank) label goes back to the value.
        r.set_class_style(&ClassRef::Value("b".into()), &style(None, None, Some("")), g).unwrap();
        r.set_class_style(&ClassRef::Other, &style(None, None, Some("  ")), g).unwrap();
        assert_eq!(labels_of(&r), ["a", "b", OTHER_VALUES]);
        // A class that isn't there, or named the wrong way, is refused and changes nothing.
        let before = r.clone();
        assert!(r.set_class_style(&ClassRef::Value("nope".into()), &style(Some(red), None, None), g).is_err());
        assert!(r.set_class_style(&ClassRef::Index(0), &style(Some(red), None, None), g).is_err());
        assert_eq!(r, before);
        assert!(Renderer::Single(s(0)).set_class_style(&ClassRef::Other, &style(Some(red), None, None), g).is_err());
        // A line class takes the colour as its line colour.
        let line = Symbol::default_for(GeometryType::Polyline, 0);
        let mut lines = unique(&[("a", line)], line, Ramp::Ember);
        let e = lines.set_class_style(&ClassRef::Value("a".into()), &style(Some(red), None, None), GeometryType::Polyline).unwrap();
        assert_eq!(e.symbol.stroke, red);
        // Graduated Colors: by number, and the label replaces the range.
        let mut gr = graduated(&[10.0, 20.0], &[s(0), s(1)], Ramp::Ember);
        let e = gr.set_class_style(&ClassRef::Index(1), &style(Some(red), None, Some("High")), g).unwrap();
        assert_eq!((e.label.as_str(), e.symbol), ("High", s(1).with_fill(red, g)));
        assert_eq!(labels_of(&gr), ["0 – 10", "High"]);
        gr.set_class_style(&ClassRef::Index(0), &style(None, None, Some("0 – 10")), g).unwrap();
        assert!(matches!(&gr, Renderer::Graduated { class_labels, .. } if !class_labels.contains_key(&0)), "the range itself is no label");
        assert!(gr.set_class_style(&ClassRef::Index(2), &style(None, None, Some("x")), g).is_err());
        assert!(gr.set_class_style(&ClassRef::Other, &style(None, None, Some("x")), g).is_err());
        // A very long label is cut.
        let e = gr.set_class_style(&ClassRef::Index(0), &style(None, None, Some(&"x".repeat(500))), g).unwrap();
        assert_eq!(e.label.chars().count(), MAX_LABEL);
        // A label is one line.
        let e = gr.set_class_style(&ClassRef::Index(0), &style(None, None, Some("Low\nincome\t")), g).unwrap();
        assert_eq!(e.label, "Low income");
        // A cut that lands on a space doesn't leave it at the end.
        let long = format!("{} {}", "x".repeat(MAX_LABEL - 1), "y".repeat(10));
        let e = gr.set_class_style(&ClassRef::Index(0), &style(None, None, Some(&long)), g).unwrap();
        assert_eq!(e.label, "x".repeat(MAX_LABEL - 1));
    }

    #[test]
    fn class_labels_are_saved_and_older_renderers_still_load() {
        let g = GeometryType::Point;
        let s = |c| Symbol::default_for(g, c);
        // A renderer nobody edited saves as before #314, so older versions still read it.
        let mut r = unique(&[("a", s(0))], s(1), Ramp::Ember);
        let j = serde_json::to_string(&r).unwrap();
        assert!(!j.contains("class_labels") && !j.contains("other_label"), "{j}");
        assert_eq!(serde_json::from_str::<Renderer>(&j).unwrap(), r);
        r.set_class_style(&ClassRef::Value("a".into()), &ClassStyle { label: Some("A".into()), ..Default::default() }, g).unwrap();
        r.set_class_style(&ClassRef::Other, &ClassStyle { label: Some("Rest".into()), ..Default::default() }, g).unwrap();
        assert_eq!(serde_json::from_str::<Renderer>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
        // Class numbers round-trip as JSON object keys.
        let mut gr = graduated(&[1.0, 2.0], &[s(0), s(1)], Ramp::Ember);
        gr.set_class_style(&ClassRef::Index(1), &ClassStyle { label: Some("Two".into()), ..Default::default() }, g).unwrap();
        assert_eq!(serde_json::from_str::<Renderer>(&serde_json::to_string(&gr).unwrap()).unwrap(), gr);
    }

    #[test]
    fn applying_the_symbology_again_keeps_hand_edits() {
        let g = GeometryType::Polygon;
        let base = Symbol::default_for(g, 0);
        // Classes coloured by their scheme, as Renderer::unique and Renderer::graduated make them.
        let ramped = |keys: &[&str], ramp: Ramp| {
            let n = keys.len();
            let classes: Vec<(&str, Symbol)> = keys.iter().enumerate().map(|(k, v)| (*v, base.with_fill(ramp.pick(k, n), g))).collect();
            unique(&classes, base.with_fill(OTHER_FILL, g), ramp)
        };
        let (red, blue) = (Rgba::rgb(0xC0, 0x30, 0x20), Rgba::rgb(0, 0, 0xFF));
        let edit = |r: &mut Renderer, c: ClassRef, color: Option<Rgba>, outline: Option<Rgba>, label: Option<&str>| {
            r.set_class_style(&c, &ClassStyle { color, outline, label: label.map(str::to_string) }, g).unwrap();
        };
        let mut old = ramped(&["a", "b", "c"], Ramp::Ember);
        edit(&mut old, ClassRef::Value("b".into()), Some(red), None, Some("Bee"));
        edit(&mut old, ClassRef::Value("c".into()), None, Some(blue), None);
        edit(&mut old, ClassRef::Other, Some(red), None, Some("Rest"));
        // Same field and scheme, "a" gone and "d" new: only the hand edits carry over. "c" keeps
        // its outline but takes the scheme's colour at its new place, not the one it had.
        let fresh = ramped(&["b", "c", "d"], Ramp::Ember);
        let mut again = fresh.clone();
        again.carry_edits_from(&old, g);
        let (Renderer::Unique { classes, other, .. }, Renderer::Unique { classes: fc, other: fo, .. }) = (&again, &fresh) else {
            panic!("unique renderers expected")
        };
        assert_eq!(classes[0].1, fc[0].1.with_fill(red, g), "b keeps its colour");
        assert_eq!((classes[1].1.fill, classes[1].1.stroke), (fc[1].1.fill, blue), "c keeps its outline only");
        assert_eq!(classes[2].1, fc[2].1, "d is new");
        assert_eq!(*other, fo.with_fill(red, g));
        assert_eq!(labels_of(&again), ["Bee", "c", "d", "Rest"]);
        // A polygon class's own transparency, and an outline that differs only in it, carry over too.
        let mut tinted = ramped(&["a"], Ramp::Ember);
        if let Renderer::Unique { classes, .. } = &mut tinted {
            classes[0].1.fill = red.with_alpha(80);
            classes[0].1.stroke = red.shade(0.6).with_alpha(100);
        }
        let mut kept = ramped(&["a"], Ramp::Ember);
        kept.carry_edits_from(&tinted, g);
        let (Renderer::Unique { classes: kc, .. }, Renderer::Unique { classes: tc, .. }) = (&kept, &tinted) else {
            panic!("unique renderers expected")
        };
        assert_eq!((kc[0].1.fill, kc[0].1.stroke), (tc[0].1.fill, tc[0].1.stroke));
        // An unedited renderer applied again is the fresh one.
        let mut plain = fresh.clone();
        plain.carry_edits_from(&ramped(&["b", "c"], Ramp::Ember), g);
        assert_eq!(plain, fresh);
        // Another scheme recolours but keeps the labels; another field starts fresh.
        let fresh = ramped(&["b"], Ramp::Harbour);
        let mut recoloured = fresh.clone();
        recoloured.carry_edits_from(&old, g);
        let (Renderer::Unique { classes, other, .. }, Renderer::Unique { classes: fc, other: fo, .. }) = (&recoloured, &fresh) else {
            panic!("unique renderers expected")
        };
        assert_eq!((classes, other), (fc, fo));
        assert_eq!(labels_of(&recoloured), ["Bee", "Rest"]);
        let mut other_field = ramped(&["b"], Ramp::Ember);
        if let Renderer::Unique { field, .. } = &mut other_field {
            *field = "h".into();
        }
        let fresh = other_field.clone();
        other_field.carry_edits_from(&old, g);
        assert_eq!(other_field, fresh);
        // Graduated Colors: edits by number while the count and scheme stay; labels while the ranges do.
        let ramped = |breaks: &[f64], ramp: Ramp| {
            let n = breaks.len();
            graduated(breaks, &(0..n).map(|i| base.with_fill(ramp.pick(i, n), g)).collect::<Vec<_>>(), ramp)
        };
        let mut gold = ramped(&[10.0, 20.0], Ramp::Ember);
        edit(&mut gold, ClassRef::Index(0), Some(red), None, Some("Low"));
        let mut same = ramped(&[10.0, 20.0], Ramp::Ember);
        same.carry_edits_from(&gold, g);
        assert_eq!(same, gold);
        let fresh = ramped(&[12.0, 20.0], Ramp::Ember);
        let mut moved = fresh.clone();
        moved.carry_edits_from(&gold, g);
        let (Renderer::Graduated { symbols, .. }, Renderer::Graduated { symbols: fs, .. }) = (&moved, &fresh) else {
            panic!("graduated renderers expected")
        };
        assert_eq!((symbols[0], symbols[1]), (fs[0], fs[1]), "the range moved: its colour goes");
        assert_eq!(labels_of(&moved), ["0 – 12", "12 – 20"], "and its label too");
        let fresh = ramped(&[10.0, 15.0, 20.0], Ramp::Ember);
        let mut more = fresh.clone();
        more.carry_edits_from(&gold, g);
        assert_eq!(more, fresh, "another number of classes starts fresh");
    }
}
