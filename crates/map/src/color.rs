//! Colours and colour ramps.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba([r, g, b, a])
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
    pub const TRANSPARENT: Rgba = Rgba([0, 0, 0, 0]);
    pub fn r(self) -> u8 {
        self.0[0]
    }
    pub fn g(self) -> u8 {
        self.0[1]
    }
    pub fn b(self) -> u8 {
        self.0[2]
    }
    pub fn a(self) -> u8 {
        self.0[3]
    }
    pub fn with_alpha(self, a: u8) -> Rgba {
        Rgba([self.0[0], self.0[1], self.0[2], a])
    }
    pub fn lerp(self, o: Rgba, t: f64) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round().clamp(0.0, 255.0) as u8;
        Rgba([m(self.0[0], o.0[0]), m(self.0[1], o.0[1]), m(self.0[2], o.0[2]), m(self.0[3], o.0[3])])
    }
    /// Darker (f < 1) or lighter (f > 1) version.
    pub fn shade(self, f: f64) -> Rgba {
        let s = |c: u8| {
            let c = c as f64;
            (if f < 1.0 { c * f } else { c + (255.0 - c) * (f - 1.0) }).round().clamp(0.0, 255.0) as u8
        };
        Rgba([s(self.0[0]), s(self.0[1]), s(self.0[2]), self.0[3]])
    }
    /// `#RRGGBB` or `#RRGGBBAA`.
    pub fn hex(self) -> String {
        if self.a() == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r(), self.g(), self.b())
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r(), self.g(), self.b(), self.a())
        }
    }
    /// Always `#RRGGBBAA`, so a fully opaque colour still carries its alpha.
    pub fn hex8(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.r(), self.g(), self.b(), self.a())
    }
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
        match s.len() {
            6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }
}

/// What a colour ramp is for, which decides how its colours are handed out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RampKind {
    /// Low to high: interpolated between stops.
    #[default]
    Sequential,
    /// Two directions from a midpoint: interpolated between stops.
    Diverging,
    /// Unordered categories: each class takes the next stop, cycling.
    Qualitative,
}

/// Most stops a custom ramp keeps.
pub const MAX_RAMP_STOPS: usize = 256;

/// A colour ramp defined by the user (or a `.rgisramp` file): a name and two or more stops,
/// each a position in 0..=1 and a colour, sorted by position. It travels inside the renderers
/// that use it, so a layer file or package keeps its colours without the project's ramp list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CustomRampFile", into = "CustomRampFile")]
pub struct CustomRamp {
    name: String,
    kind: RampKind,
    stops: Vec<(f64, Rgba)>,
}

/// A custom ramp as files hold it: `stops` are colours (`"#RRGGBB"` or `"#RRGGBBAA"`), evenly
/// spaced, or `{"pos": 0..1, "color": "#…"}` for uneven spacing.
#[derive(Serialize, Deserialize)]
struct CustomRampFile {
    name: String,
    #[serde(default)]
    kind: RampKind,
    stops: Vec<StopFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StopFile {
    Color(String),
    At { pos: f64, color: String },
}

impl TryFrom<CustomRampFile> for CustomRamp {
    type Error = String;
    fn try_from(f: CustomRampFile) -> Result<Self, String> {
        let mut stops = Vec::with_capacity(f.stops.len().min(MAX_RAMP_STOPS));
        for s in f.stops.iter().take(MAX_RAMP_STOPS + 1) {
            let (pos, color) = match s {
                StopFile::Color(c) => (None, c),
                StopFile::At { pos, color } => (Some(*pos), color),
            };
            let c = Rgba::from_hex(color).ok_or_else(|| format!("'{color}' is not a colour (#RRGGBB or #RRGGBBAA)"))?;
            stops.push((pos, c));
        }
        CustomRamp::new(&f.name, f.kind, stops)
    }
}

impl From<CustomRamp> for CustomRampFile {
    fn from(r: CustomRamp) -> Self {
        let n = r.stops.len();
        let even = r.stops.iter().enumerate().all(|(i, (p, _))| (p - i as f64 / (n - 1).max(1) as f64).abs() < 1e-9);
        let stops = r
            .stops
            .into_iter()
            .map(|(pos, c)| if even { StopFile::Color(c.hex()) } else { StopFile::At { pos, color: c.hex() } })
            .collect();
        CustomRampFile { name: r.name, kind: r.kind, stops }
    }
}

impl CustomRamp {
    /// A ramp from its stops: a stop without a position is spaced evenly between its
    /// neighbours' (the first and last default to 0 and 1). Positions must be finite, within
    /// 0..=1 and ascending.
    pub fn new(name: &str, kind: RampKind, stops: Vec<(Option<f64>, Rgba)>) -> Result<CustomRamp, String> {
        let name: String = name.trim().chars().take(200).collect();
        if name.is_empty() {
            return Err("a colour ramp needs a name".into());
        }
        let min = if kind == RampKind::Qualitative { 1 } else { 2 };
        if stops.len() < min || stops.len() > MAX_RAMP_STOPS {
            return Err(format!("{name}: a colour ramp has {min} to {MAX_RAMP_STOPS} stops, not {}", stops.len()));
        }
        let n = stops.len();
        let mut pos: Vec<Option<f64>> = stops.iter().map(|s| s.0).collect();
        if let Some(Some(p)) = pos.iter().find(|p| p.is_some_and(|p| !(p.is_finite() && (0.0..=1.0).contains(&p)))) {
            return Err(format!("{name}: stop position {p} is outside 0 to 1"));
        }
        if let Some(first) = pos.first_mut() {
            first.get_or_insert(0.0);
        }
        if let Some(last) = pos.last_mut() {
            last.get_or_insert(1.0);
        }
        // Fill each run of missing positions evenly between the known ones around it.
        let mut i = 0;
        while i < n {
            if pos.get(i).copied().flatten().is_some() {
                i += 1;
                continue;
            }
            let a = i - 1;
            let b = (i..n).find(|&k| pos.get(k).copied().flatten().is_some()).unwrap_or(n - 1);
            let (pa, pb) = (pos.get(a).copied().flatten().unwrap_or(0.0), pos.get(b).copied().flatten().unwrap_or(1.0));
            for k in i..b {
                if let Some(p) = pos.get_mut(k) {
                    *p = Some(pa + (pb - pa) * (k - a) as f64 / (b - a) as f64);
                }
            }
            i = b + 1;
        }
        let stops: Vec<(f64, Rgba)> = pos.into_iter().zip(stops).map(|(p, (_, c))| (p.unwrap_or(0.0), c)).collect();
        if stops.windows(2).any(|w| matches!(w, [a, b] if b.0 < a.0)) {
            return Err(format!("{name}: stop positions must go up from 0 to 1"));
        }
        Ok(CustomRamp { name, kind, stops })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name, to make it unique among a project's ramps (keep it non-empty).
    pub fn name_mut(&mut self) -> &mut String {
        &mut self.name
    }

    pub fn kind(&self) -> RampKind {
        self.kind
    }

    /// The stops, as (position, colour), sorted by position.
    pub fn stops(&self) -> &[(f64, Rgba)] {
        &self.stops
    }

    fn at(&self, t: f64) -> Rgba {
        let s = &self.stops;
        let (Some(first), Some(last)) = (s.first(), s.last()) else { return Rgba::rgb(128, 128, 128) };
        if self.kind == RampKind::Qualitative {
            let k = (t * (s.len() - 1) as f64).round() as usize;
            return s.get(k).map_or(last.1, |c| c.1);
        }
        if t <= first.0 {
            return first.1;
        }
        for w in s.windows(2) {
            if let [(p0, c0), (p1, c1)] = w
                && t <= *p1
            {
                return if p1 > p0 { c0.lerp(*c1, (t - p0) / (p1 - p0)) } else { *c1 };
            }
        }
        last.1
    }
}

/// A colour ramp: one of our own (sequential, diverging and qualitative), or a custom one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Ramp {
    #[default]
    Ember,
    Lagoon,
    Forest,
    Dusk,
    Thermal,
    Diverging,
    Greys,
    Harbour,
    /// Lowland green through tan to snow, for elevation.
    Terrain,
    /// Deep indigo through teal to bright yellow, perceptually even.
    Aurora,
    /// A ramp defined by the user.
    Custom(Box<CustomRamp>),
}

impl Ramp {
    pub const ALL: [Ramp; 10] = [
        Ramp::Ember,
        Ramp::Lagoon,
        Ramp::Forest,
        Ramp::Dusk,
        Ramp::Thermal,
        Ramp::Diverging,
        Ramp::Greys,
        Ramp::Terrain,
        Ramp::Aurora,
        Ramp::Harbour,
    ];

    /// The name commands take (`"Ember"`, or a custom ramp's own name).
    pub fn name(&self) -> &str {
        match self {
            Ramp::Ember => "Ember",
            Ramp::Lagoon => "Lagoon",
            Ramp::Forest => "Forest",
            Ramp::Dusk => "Dusk",
            Ramp::Thermal => "Thermal",
            Ramp::Diverging => "Diverging",
            Ramp::Greys => "Greys",
            Ramp::Harbour => "Harbour",
            Ramp::Terrain => "Terrain",
            Ramp::Aurora => "Aurora",
            Ramp::Custom(c) => c.name(),
        }
    }

    pub fn kind(&self) -> RampKind {
        match self {
            Ramp::Harbour => RampKind::Qualitative,
            Ramp::Diverging => RampKind::Diverging,
            Ramp::Custom(c) => c.kind(),
            _ => RampKind::Sequential,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Ramp::Ember => "Ember",
            Ramp::Lagoon => "Lagoon",
            Ramp::Forest => "Forest",
            Ramp::Dusk => "Dusk",
            Ramp::Thermal => "Thermal",
            Ramp::Diverging => "Red–Blue",
            Ramp::Greys => "Greys",
            Ramp::Harbour => "Harbour (qualitative)",
            Ramp::Terrain => "Terrain",
            Ramp::Aurora => "Aurora",
            Ramp::Custom(c) => c.name(),
        }
    }

    /// One of our own ramps by name or label (custom ramps are looked up in the project).
    pub fn from_name(s: &str) -> Option<Ramp> {
        Ramp::ALL.into_iter().find(|r| r.name().eq_ignore_ascii_case(s.trim()) || r.label().eq_ignore_ascii_case(s.trim()))
    }

    fn stops(&self) -> &'static [[u8; 3]] {
        match self {
            Ramp::Ember => &[[0xFF, 0xF3, 0xD6], [0xFD, 0xC9, 0x7A], [0xF5, 0x8A, 0x4B], [0xD9, 0x48, 0x3B], [0x8E, 0x1B, 0x3A]],
            Ramp::Lagoon => &[[0xEE, 0xF8, 0xF6], [0xA6, 0xDD, 0xD4], [0x52, 0xB3, 0xB6], [0x22, 0x7C, 0x9D], [0x17, 0x3F, 0x6E]],
            Ramp::Forest => &[[0xF4, 0xF9, 0xE8], [0xC2, 0xE3, 0x9C], [0x7B, 0xC0, 0x6A], [0x35, 0x8E, 0x4C], [0x10, 0x55, 0x33]],
            Ramp::Dusk => &[[0xF6, 0xEF, 0xF7], [0xD4, 0xB9, 0xDA], [0xB0, 0x7A, 0xB8], [0x7E, 0x45, 0x9A], [0x45, 0x1D, 0x6B]],
            Ramp::Thermal => &[[0x2C, 0x3E, 0x8C], [0x2E, 0x9F, 0xC0], [0xA8, 0xDA, 0x6A], [0xF9, 0xC7, 0x3A], [0xD8, 0x3A, 0x2A]],
            Ramp::Diverging => &[[0xB2, 0x2B, 0x35], [0xEE, 0x9A, 0x80], [0xF5, 0xF2, 0xEF], [0x8A, 0xB8, 0xDB], [0x2A, 0x5C, 0x9C]],
            Ramp::Greys => &[[0xF4, 0xF4, 0xF4], [0xC4, 0xC4, 0xC4], [0x8E, 0x8E, 0x8E], [0x55, 0x55, 0x55], [0x22, 0x22, 0x22]],
            Ramp::Terrain => {
                &[[0x2E, 0x7D, 0x4F], [0x8F, 0xBF, 0x6A], [0xE8, 0xD9, 0x8C], [0xB0, 0x82, 0x55], [0x8A, 0x6E, 0x5C], [0xF7, 0xF5, 0xF2]]
            }
            Ramp::Aurora => {
                &[[0x2B, 0x16, 0x5E], [0x3A, 0x4E, 0x9A], [0x23, 0x8A, 0x96], [0x3C, 0xB6, 0x7C], [0xA6, 0xD8, 0x4A], [0xF7, 0xE6, 0x3B]]
            }
            Ramp::Harbour => &[
                [0x3B, 0x7D, 0xD8],
                [0xE8, 0x7A, 0x41],
                [0x4C, 0xAF, 0x7A],
                [0xC9, 0x4F, 0x8C],
                [0xE3, 0xBE, 0x3C],
                [0x6F, 0x5B, 0xC9],
                [0x3F, 0xB5, 0xC2],
                [0xA6, 0x6E, 0x4E],
                [0x8D, 0x9B, 0x3A],
                [0xD9, 0x5D, 0x5D],
            ],
            // Sampled from its own stops (`CustomRamp::at`).
            Ramp::Custom(_) => &[],
        }
    }

    pub fn is_qualitative(&self) -> bool {
        self.kind() == RampKind::Qualitative
    }

    /// Colour at `t` ∈ [0, 1] (qualitative ramps pick the nearest swatch).
    pub fn at(&self, t: f64) -> Rgba {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        if let Ramp::Custom(c) = self {
            return c.at(t);
        }
        let s = self.stops();
        let n = s.len();
        if n == 0 {
            return Rgba::rgb(128, 128, 128);
        }
        let f = t * (n - 1) as f64;
        let i = (f.floor() as usize).min(n - 1);
        let j = (i + 1).min(n - 1);
        let c = |k: usize| s.get(k).map(|c| Rgba::rgb(c[0], c[1], c[2])).unwrap_or(Rgba::rgb(128, 128, 128));
        if self.is_qualitative() { c(f.round() as usize) } else { c(i).lerp(c(j), f - i as f64) }
    }

    /// The `k`-th of `n` evenly spaced colours (qualitative ramps cycle). A custom ramp with
    /// exactly `n` stops hands out its stops as they are, so a stepped palette keeps its colours.
    pub fn pick(&self, k: usize, n: usize) -> Rgba {
        if let Ramp::Custom(c) = self {
            let s = c.stops();
            if c.kind() == RampKind::Qualitative || s.len() == n {
                return s.get(k % s.len().max(1)).map_or(Rgba::rgb(128, 128, 128), |c| c.1);
            }
        }
        if self.is_qualitative() {
            let s = self.stops();
            let c = s.get(k % s.len().max(1)).copied().unwrap_or([128, 128, 128]);
            return Rgba::rgb(c[0], c[1], c[2]);
        }
        if n <= 1 { self.at(0.6) } else { self.at(k as f64 / (n - 1) as f64) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_ramps() {
        let c = Rgba::new(1, 2, 255, 128);
        assert_eq!(Rgba::from_hex(&c.hex()), Some(c));
        assert_eq!(Rgba::from_hex("#zz0000"), None);
        assert_eq!(Rgba::from_hex("#ééé"), None);
        for r in Ramp::ALL {
            let _ = r.at(f64::NAN);
            let _ = r.pick(99, 3);
            let _ = r.pick(0, 0);
            assert_eq!(Ramp::from_name(r.name()), Some(r.clone()));
        }
    }

    fn custom(json: &str) -> Result<CustomRamp, String> {
        serde_json::from_str::<CustomRamp>(json).map_err(|e| e.to_string())
    }

    #[test]
    fn custom_ramp_files() {
        // The issue's example: evenly spaced hex stops.
        let r = custom(r##"{"name": " Phosphor Frostfire ", "kind": "diverging", "stops": ["#eff6ff", "#abc5e8", "#7b5ca6"]}"##).unwrap();
        assert_eq!(r.name(), "Phosphor Frostfire");
        assert_eq!(r.kind(), RampKind::Diverging);
        assert_eq!(r.stops().iter().map(|s| s.0).collect::<Vec<_>>(), [0.0, 0.5, 1.0]);
        let ramp = Ramp::Custom(Box::new(r.clone()));
        assert_eq!(ramp.at(0.0), Rgba::rgb(0xef, 0xf6, 0xff));
        assert_eq!(ramp.at(0.25), Rgba::rgb(0xef, 0xf6, 0xff).lerp(Rgba::rgb(0xab, 0xc5, 0xe8), 0.5));
        assert_eq!(ramp.at(f64::NAN), Rgba::rgb(0xef, 0xf6, 0xff));
        // Written back the way it was read.
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains(r##""stops":["#EFF6FF","#ABC5E8","#7B5CA6"]"##), "{text}");
        assert_eq!(custom(&text).unwrap(), r);

        // Uneven stops, mixed with plain ones: the gap is filled evenly.
        let r = custom(r##"{"name": "u", "stops": ["#000000", {"pos": 0.8, "color": "#ffffff80"}, "#ff0000"]}"##).unwrap();
        assert_eq!(r.kind(), RampKind::Sequential);
        assert_eq!(r.stops().iter().map(|s| s.0).collect::<Vec<_>>(), [0.0, 0.8, 1.0]);
        assert_eq!(r.stops().get(1).map(|s| s.1), Some(Rgba::new(255, 255, 255, 128)));
        let r2 =
            custom(r##"{"name": "g", "stops": ["#000000", "#111111", "#222222", {"pos": 0.9, "color": "#333333"}, "#444444"]}"##).unwrap();
        let p: Vec<f64> = r2.stops().iter().map(|s| s.0).collect();
        assert!((p[1] - 0.3).abs() < 1e-9 && (p[2] - 0.6).abs() < 1e-9 && p[3] == 0.9, "{p:?}");
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains(r#""pos":0.8"#), "{text}");
        assert_eq!(custom(&text).unwrap(), r);

        // A stepped ramp with as many stops as classes keeps its colours, whatever their spacing.
        let ramp = Ramp::Custom(Box::new(r));
        assert_eq!(
            (0..3).map(|k| ramp.pick(k, 3)).collect::<Vec<_>>(),
            [Rgba::rgb(0, 0, 0), Rgba::new(255, 255, 255, 128), Rgba::rgb(255, 0, 0)]
        );
        // Qualitative ramps cycle through their stops.
        let q = Ramp::Custom(Box::new(custom(r##"{"name": "q", "kind": "qualitative", "stops": ["#111111"]}"##).unwrap()));
        assert!(q.is_qualitative());
        assert_eq!(q.pick(5, 9), Rgba::rgb(0x11, 0x11, 0x11));
        assert_eq!(q.at(0.7), Rgba::rgb(0x11, 0x11, 0x11));
    }

    #[test]
    fn custom_ramps_refuse_bad_input() {
        for bad in [
            r##"{"name": "", "stops": ["#000000", "#ffffff"]}"##,
            r##"{"name": "one", "stops": ["#000000"]}"##,
            r##"{"name": "none", "kind": "qualitative", "stops": []}"##,
            r##"{"name": "colour", "stops": ["#000000", "red"]}"##,
            r##"{"name": "short", "stops": ["#000", "#fff"]}"##,
            r##"{"name": "range", "stops": [{"pos": -0.1, "color": "#000000"}, "#ffffff"]}"##,
            r##"{"name": "nan", "stops": [{"pos": 1e999, "color": "#000000"}, "#ffffff"]}"##,
            r##"{"name": "order", "stops": ["#000000", {"pos": 0.8, "color": "#111111"}, {"pos": 0.2, "color": "#222222"}, "#ffffff"]}"##,
            r##"{"name": "kind", "kind": "rainbow", "stops": ["#000000", "#ffffff"]}"##,
        ] {
            assert!(custom(bad).is_err(), "{bad}");
        }
        let many = format!(r#"{{"name": "many", "stops": [{}]}}"#, vec![r##""#000000""##; MAX_RAMP_STOPS + 1].join(","));
        assert!(custom(&many).is_err());
        let most = format!(r#"{{"name": "most", "stops": [{}]}}"#, vec![r##""#000000""##; MAX_RAMP_STOPS].join(","));
        assert!(custom(&most).is_ok());
    }
}
