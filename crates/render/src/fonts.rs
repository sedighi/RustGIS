//! Font discovery: a good installed sans-serif when there is one (Segoe UI, SF, Inter, Noto Sans,
//! Open Sans, DejaVu Sans), else the Ubuntu Light face that ships with egui. Fonts are read at
//! run time and never bundled.

use std::sync::OnceLock;

/// Candidate files for (regular, semibold), in order of preference.
const CANDIDATES: &[(&str, &str)] = &[
    ("C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\seguisb.ttf"),
    ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/SFNS.ttf"),
    ("/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ("/usr/share/fonts/TTF/Inter-Regular.ttf", "/usr/share/fonts/TTF/Inter-SemiBold.ttf"),
    ("/usr/share/fonts/inter/Inter-Regular.ttf", "/usr/share/fonts/inter/Inter-SemiBold.ttf"),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", "/usr/share/fonts/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf", "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/TTF/OpenSans-Regular.ttf", "/usr/share/fonts/TTF/OpenSans-SemiBold.ttf"),
    ("/usr/share/fonts/truetype/open-sans/OpenSans-Regular.ttf", "/usr/share/fonts/truetype/open-sans/OpenSans-SemiBold.ttf"),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
];

/// Largest font file accepted (32 MiB).
const MAX_FONT: u64 = 32 << 20;

pub struct UiFonts {
    pub regular: Vec<u8>,
    pub semibold: Vec<u8>,
    /// File the regular face came from, or "built-in".
    pub source: String,
}

fn read(path: &str) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FONT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    ab_glyph::FontRef::try_from_slice(&bytes).ok()?;
    Some(bytes)
}

/// The interface fonts, discovered once.
pub fn ui_fonts() -> &'static UiFonts {
    static F: OnceLock<UiFonts> = OnceLock::new();
    F.get_or_init(|| {
        if std::env::var_os("RUSTGIS_BUILTIN_FONTS").is_none() {
            for (r, b) in CANDIDATES {
                if let Some(regular) = read(r) {
                    let semibold = read(b).unwrap_or_else(|| regular.clone());
                    return UiFonts { regular, semibold, source: (*r).to_string() };
                }
            }
        }
        let b = epaint_default_fonts::UBUNTU_LIGHT.to_vec();
        UiFonts { regular: b.clone(), semibold: b, source: "built-in".into() }
    })
}

/// Regional CJK or extra script glyph styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CjkRegion {
    SimplifiedChinese,
    Japanese,
    Korean,
    Persian,
}

/// Installed CJK or extra script font candidates: (path, face index in a collection).
fn cjk_candidates(region: CjkRegion) -> &'static [(&'static str, u32)] {
    // Noto Sans CJK collections order their faces JP, KR, SC, TC, HK.
    match region {
        CjkRegion::SimplifiedChinese => &[
            ("C:\\Windows\\Fonts\\msyh.ttc", 0),
            ("/System/Library/Fonts/PingFang.ttc", 0),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/noto-cjk/NotoSansSC-Regular.otf", 0),
            ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0),
        ],
        CjkRegion::Japanese => &[
            ("C:\\Windows\\Fonts\\YuGothR.ttc", 0),
            ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
            ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansJP-Regular.otf", 0),
        ],
        CjkRegion::Korean => &[
            ("C:\\Windows\\Fonts\\malgun.ttf", 0),
            ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/noto-cjk/NotoSansKR-Regular.otf", 0),
        ],
        CjkRegion::Persian => &[
            ("fonts/Vazirmatn-Regular.ttf", 0),
            ("apps/rustgis-web/fonts/Vazirmatn-Regular.ttf", 0),
            ("C:\\Windows\\Fonts\\Vazirmatn-Regular.ttf", 0),
            ("C:\\Windows\\Fonts\\tahoma.ttf", 0),
            ("C:\\Windows\\Fonts\\arial.ttf", 0),
            ("C:\\Windows\\Fonts\\seguisym.ttf", 0),
            ("/usr/share/fonts/vazirmatn/Vazirmatn-Regular.ttf", 0),
            ("/usr/share/fonts/truetype/vazirmatn/Vazirmatn-Regular.ttf", 0),
            ("/usr/share/fonts/noto/NotoSansArabic-Regular.ttf", 0),
            ("/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf", 0),
            ("/usr/share/fonts/google-noto/NotoSansArabic-Regular.ttf", 0),
            ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
            ("/System/Library/Fonts/GeezaPro.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Arial.ttf", 0),
            ("/Library/Fonts/Vazirmatn-Regular.ttf", 0),
        ],
    }
}

/// An installed CJK or script font for `region` (bytes and face index), found once. `None` on the
/// web or when nothing suitable is installed.
pub fn cjk_font(region: CjkRegion) -> Option<&'static (Vec<u8>, u32)> {
    static FOUND: [OnceLock<Option<(Vec<u8>, u32)>>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = match region {
        CjkRegion::SimplifiedChinese => &FOUND[0],
        CjkRegion::Japanese => &FOUND[1],
        CjkRegion::Korean => &FOUND[2],
        CjkRegion::Persian => &FOUND[3],
    };
    slot.get_or_init(|| {
        cjk_candidates(region).iter().find_map(|(path, index)| {
            let meta = std::fs::metadata(path).ok()?;
            // CJK collections are large; accept up to 128 MiB.
            if meta.len() > 128 << 20 {
                return None;
            }
            std::fs::read(path).ok().filter(|b| valid_font(b, *index)).map(|b| (b, *index))
        })
    })
    .as_ref()
}

/// Whether `bytes` hold a font with face `index` that the font parser accepts. Fonts handed to the
/// UI are checked first: a corrupt file or an out-of-range face index must never reach it.
pub fn valid_font(bytes: &[u8], index: u32) -> bool {
    ab_glyph::FontRef::try_from_slice_and_index(bytes, index).is_ok()
}

/// Where the web app downloads a script face from (Noto Sans / Vazirmatn, SIL Open Font License).
pub fn cjk_font_url(region: CjkRegion) -> &'static str {
    match region {
        CjkRegion::SimplifiedChinese => "https://assets.rustgis.app/fonts/NotoSansSC-Regular.otf",
        CjkRegion::Japanese => "https://assets.rustgis.app/fonts/NotoSansJP-Regular.otf",
        CjkRegion::Korean => "https://assets.rustgis.app/fonts/NotoSansKR-Regular.otf",
        CjkRegion::Persian => "fonts/Vazirmatn-Regular.ttf",
    }
}
