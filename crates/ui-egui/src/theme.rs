//! Design tokens (light and dark), interface fonts and the egui style.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

/// RustGIS brand colour (the Project tab and the logo) and its lighter tint.
pub const BRAND: Color32 = Color32::from_rgb(0xB8, 0x4A, 0x2C);
pub const BRAND_DARK: Color32 = Color32::from_rgb(0x96, 0x3A, 0x22);
/// Contextual tab group colour (Feature Layer, Labeling, Data).
pub const CONTEXT: Color32 = Color32::from_rgb(0x0E, 0x93, 0x84);

/// Every colour the interface uses; widgets never hard-code colours.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub header: Color32,
    pub ribbon: Color32,
    pub tab_strip: Color32,
    pub canvas: Color32,
    pub pane: Color32,
    pub pane_header: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub checked: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub input: Color32,
    pub input_border: Color32,
    pub group_label: Color32,
    pub status_bar: Color32,
    pub selection: Color32,
    pub menu: Color32,
    pub shadow: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
    pub context: Color32,
    pub context_tint: Color32,
}

impl Tokens {
    pub fn light() -> Self {
        Tokens {
            dark: false,
            header: Color32::from_rgb(0xEE, 0xF1, 0xF5),
            ribbon: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            tab_strip: Color32::from_rgb(0xEE, 0xF1, 0xF5),
            canvas: Color32::from_rgb(0xE4, 0xE8, 0xEE),
            pane: Color32::from_rgb(0xFB, 0xFC, 0xFD),
            pane_header: Color32::from_rgb(0xF3, 0xF5, 0xF8),
            border: Color32::from_rgb(0xDD, 0xE2, 0xE9),
            border_strong: Color32::from_rgb(0xC3, 0xCA, 0xD4),
            text: Color32::from_rgb(0x1F, 0x26, 0x30),
            text_dim: Color32::from_rgb(0x5E, 0x69, 0x77),
            text_disabled: Color32::from_rgb(0xA9, 0xB1, 0xBC),
            icon: Color32::from_rgb(0x3A, 0x45, 0x54),
            hover: Color32::from_rgb(0xE9, 0xF0, 0xFB),
            pressed: Color32::from_rgb(0xD4, 0xE2, 0xF8),
            checked: Color32::from_rgb(0xDC, 0xE8, 0xFA),
            accent: Color32::from_rgb(0x25, 0x63, 0xEB),
            accent_text: Color32::from_rgb(0x1D, 0x4E, 0xC4),
            on_accent: Color32::WHITE,
            input: Color32::WHITE,
            input_border: Color32::from_rgb(0xC6, 0xCD, 0xD7),
            group_label: Color32::from_rgb(0x70, 0x7A, 0x88),
            status_bar: Color32::from_rgb(0xF3, 0xF5, 0xF8),
            selection: Color32::from_rgb(0xCF, 0xE0, 0xFC),
            menu: Color32::WHITE,
            shadow: Color32::from_black_alpha(28),
            green: Color32::from_rgb(0x16, 0x9A, 0x5A),
            red: Color32::from_rgb(0xD1, 0x3C, 0x3C),
            amber: Color32::from_rgb(0xE5, 0x9E, 0x1A),
            context: CONTEXT,
            context_tint: Color32::from_rgb(0xE3, 0xF5, 0xF2),
        }
    }

    pub fn dark() -> Self {
        Tokens {
            dark: true,
            header: Color32::from_rgb(0x17, 0x1B, 0x21),
            ribbon: Color32::from_rgb(0x24, 0x29, 0x31),
            tab_strip: Color32::from_rgb(0x17, 0x1B, 0x21),
            canvas: Color32::from_rgb(0x12, 0x15, 0x1A),
            pane: Color32::from_rgb(0x1E, 0x23, 0x2A),
            pane_header: Color32::from_rgb(0x24, 0x29, 0x31),
            border: Color32::from_rgb(0x34, 0x3B, 0x45),
            border_strong: Color32::from_rgb(0x48, 0x51, 0x5D),
            text: Color32::from_rgb(0xE6, 0xEA, 0xEF),
            text_dim: Color32::from_rgb(0x9A, 0xA4, 0xB1),
            text_disabled: Color32::from_rgb(0x5C, 0x65, 0x71),
            icon: Color32::from_rgb(0xC9, 0xD1, 0xDB),
            hover: Color32::from_rgb(0x2F, 0x37, 0x44),
            pressed: Color32::from_rgb(0x2A, 0x3D, 0x5E),
            checked: Color32::from_rgb(0x28, 0x3B, 0x5C),
            accent: Color32::from_rgb(0x5B, 0x8D, 0xF6),
            accent_text: Color32::from_rgb(0x8A, 0xAF, 0xFA),
            on_accent: Color32::WHITE,
            input: Color32::from_rgb(0x15, 0x19, 0x1F),
            input_border: Color32::from_rgb(0x44, 0x4D, 0x59),
            group_label: Color32::from_rgb(0x8C, 0x96, 0xA3),
            status_bar: Color32::from_rgb(0x1A, 0x1E, 0x24),
            selection: Color32::from_rgb(0x26, 0x40, 0x6E),
            menu: Color32::from_rgb(0x26, 0x2C, 0x34),
            shadow: Color32::from_black_alpha(110),
            green: Color32::from_rgb(0x3F, 0xC2, 0x82),
            red: Color32::from_rgb(0xF0, 0x6A, 0x6A),
            amber: Color32::from_rgb(0xF2, 0xB8, 0x48),
            context: Color32::from_rgb(0x2C, 0xC2, 0xAE),
            context_tint: Color32::from_rgb(0x1B, 0x33, 0x31),
        }
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        if ctx.global_style().visuals.dark_mode { Tokens::dark() } else { Tokens::light() }
    }
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Installs the interface fonts: an installed system sans-serif when available, then `extra`
/// faces (name, bytes, collection index) as fallbacks for scripts it lacks, such as CJK.
pub fn install_fonts(
    ctx: &egui::Context,
    extra: &[(String, Arc<FontData>, Option<rustgis_engine::render::fonts::CjkRegion>)],
    current: Option<rustgis_engine::render::fonts::CjkRegion>,
) {
    let f = rustgis_engine::render::fonts::ui_fonts();
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(f.regular.clone())));
    fonts.font_data.insert("ui-semibold".into(), Arc::new(FontData::from_owned(f.semibold.clone())));
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "ui".into());
        // egui's Hack face has the Georgian letters that Ubuntu Light and Noto Sans lack.
        prop.push("Hack".into());
    }
    let mut semi = vec!["ui-semibold".to_string(), "ui".to_string()];
    if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
        semi.extend(prop.iter().skip(1).cloned());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    for (name, data, region) in extra {
        fonts.font_data.insert(name.clone(), Arc::clone(data));
        let primary = region.is_some() && *region == current;
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into())] {
            if let Some(v) = fonts.families.get_mut(&fam) {
                if primary {
                    // Right after the interface face, so its regional glyph shapes win.
                    v.insert(1.min(v.len()), name.clone());
                } else {
                    v.push(name.clone());
                }
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.pane;
    v.window_fill = t.menu;
    v.extreme_bg_color = t.input;
    v.faint_bg_color = t.pane_header;
    v.window_stroke = Stroke::new(1.0, t.border_strong);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    // Slider rails and checkbox wells use this fill; keep it visible on white.
    v.widgets.inactive.bg_fill = if t.dark { Color32::from_rgb(0x3A, 0x42, 0x4D) } else { Color32::from_rgb(0xDD, 0xE3, 0xEA) };
    v.slider_trailing_fill = true;
    v.widgets.inactive.weak_bg_fill = t.input;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.input_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(5);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.corner_radius = CornerRadius::same(5);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.active.corner_radius = CornerRadius::same(5);
    v.widgets.open.bg_fill = t.pressed;
    v.widgets.open.weak_bg_fill = t.pressed;
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow };
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: t.shadow };
    // Pin egui's theme to ours: the app decides when to follow the system (`dark_follows_system`).
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.text_styles.insert(egui::TextStyle::Body, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Button, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Small, regular(10.5));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(12.0));
    });
}

/// egui colour from a map colour.
pub fn c32(c: rustgis_engine::map::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

pub fn rgba(c: Color32) -> rustgis_engine::map::Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    rustgis_engine::map::Rgba::new(r, g, b, a)
}
