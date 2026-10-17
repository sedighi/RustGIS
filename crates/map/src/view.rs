//! View settings: post-processing effects (illumination, weather, visual and camera effects,
//! colour adjustments), colour vision simulation, scene drawing mode, view clipping, display
//! quality, and on-screen navigation aids.

use serde::{Deserialize, Serialize};

/// Weather drawn over the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weather {
    #[default]
    None,
    Haze,
    Fog,
    Rain,
    Snow,
}

/// Simulated colour vision deficiency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorVision {
    #[default]
    Normal,
    Protanopia,
    Deuteranopia,
    Tritanopia,
    Achromatopsia,
}

impl ColorVision {
    pub const ALL: [ColorVision; 5] =
        [ColorVision::Normal, ColorVision::Protanopia, ColorVision::Deuteranopia, ColorVision::Tritanopia, ColorVision::Achromatopsia];
    pub fn code(self) -> &'static str {
        match self {
            ColorVision::Normal => "normal",
            ColorVision::Protanopia => "protanopia",
            ColorVision::Deuteranopia => "deuteranopia",
            ColorVision::Tritanopia => "tritanopia",
            ColorVision::Achromatopsia => "achromatopsia",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ColorVision::Normal => "Normal vision",
            ColorVision::Protanopia => "Protanopia (no red)",
            ColorVision::Deuteranopia => "Deuteranopia (no green)",
            ColorVision::Tritanopia => "Tritanopia (no blue)",
            ColorVision::Achromatopsia => "Achromatopsia (no colour)",
        }
    }
    pub fn from_code(s: &str) -> Option<ColorVision> {
        ColorVision::ALL.into_iter().find(|c| c.code().eq_ignore_ascii_case(s.trim()))
    }
    /// The colour matrix (linear RGB) simulating it.
    pub fn matrix(self) -> Option<[[f32; 3]; 3]> {
        // Machado, Oliveira and Fernandes (2009), severity 1.0.
        match self {
            ColorVision::Normal => None,
            ColorVision::Protanopia => {
                Some([[0.152_286, 1.052_583, -0.204_868], [0.114_503, 0.786_281, 0.099_216], [-0.003_882, -0.048_116, 1.051_998]])
            }
            ColorVision::Deuteranopia => {
                Some([[0.367_322, 0.860_646, -0.227_968], [0.280_085, 0.672_501, 0.047_413], [-0.011_820, 0.042_940, 0.968_881]])
            }
            ColorVision::Tritanopia => {
                Some([[1.255_528, -0.076_749, -0.178_779], [-0.078_411, 0.930_809, 0.147_602], [0.004_733, 0.691_367, 0.303_900]])
            }
            ColorVision::Achromatopsia => Some([[0.2126, 0.7152, 0.0722], [0.2126, 0.7152, 0.0722], [0.2126, 0.7152, 0.0722]]),
        }
    }
}

/// Effects applied to the drawn map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewEffects {
    /// Stars in the space around the globe.
    #[serde(default = "yes")]
    pub stars: bool,
    /// Daylight and night shading for this time (seconds since 1970, UTC), when set.
    #[serde(default)]
    pub sun_time: Option<f64>,
    #[serde(default)]
    pub weather: Weather,
    /// Darkened corners.
    #[serde(default)]
    pub vignette: bool,
    /// Outlines of shapes drawn over the map (an edge filter).
    #[serde(default)]
    pub edges: bool,
    /// Blur toward the top and bottom of the view, as a miniature (tilt-shift) camera.
    #[serde(default)]
    pub tilt_shift: bool,
    /// Film grain.
    #[serde(default)]
    pub grain: bool,
    /// −100 to 100.
    #[serde(default)]
    pub brightness: f64,
    #[serde(default)]
    pub contrast: f64,
    #[serde(default)]
    pub saturation: f64,
    /// 0 to 100.
    #[serde(default)]
    pub sepia: f64,
}

fn yes() -> bool {
    true
}

impl Default for ViewEffects {
    fn default() -> Self {
        ViewEffects {
            stars: true,
            sun_time: None,
            weather: Weather::None,
            vignette: false,
            edges: false,
            tilt_shift: false,
            grain: false,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            sepia: 0.0,
        }
    }
}

impl ViewEffects {
    /// No effect to apply after drawing the map (the stars are part of drawing it).
    pub fn is_none(&self) -> bool {
        ViewEffects { stars: true, ..self.clone() } == ViewEffects::default()
    }
}

/// How the map draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewSettings {
    #[serde(default)]
    pub effects: ViewEffects,
    /// Session-only (like the next few): how the screen shows the map, not part of the project.
    #[serde(skip)]
    pub color_vision: ColorVision,
    /// Polygons as outlines only (the scene's wireframe drawing mode).
    #[serde(default)]
    pub wireframe: bool,
    /// Features farther than this many metres from the view's centre aren't drawn, when set.
    #[serde(default)]
    pub clip_distance: Option<f64>,
    /// Speed over quality: no antialiasing.
    #[serde(skip)]
    pub fast: bool,
    /// The on-screen navigator (compass, zoom slider and pan pad).
    #[serde(skip)]
    pub navigator: bool,
    /// Keyboard (WASD) walking navigation instead of mouse panning. Session-only: a reopened
    /// project starts with ordinary navigation, so the keys aren't captured unexpectedly.
    #[serde(skip)]
    pub first_person: bool,
}

/// An animation keyframe: a view, and the seconds taken to reach it from the previous one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub center: rustgis_geom::Coord,
    pub scale: f64,
    #[serde(default = "two")]
    pub seconds: f64,
}

fn two() -> f64 {
    2.0
}
