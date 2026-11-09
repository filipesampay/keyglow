//! Identidade visual do keyglow.
//!
//! A interface é escura e neutra de propósito: a única cor forte é a luz
//! do próprio teclado. O destaque (seleção, sliders) acompanha a cor que o
//! teclado está mostrando, em vez de ter um tom fixo.

use crate::drivers::Rgb;
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x17, 0x1A, 0x22);
pub const PANEL: Color32 = Color32::from_rgb(0x1F, 0x23, 0x2D);
pub const RAISED: Color32 = Color32::from_rgb(0x2A, 0x2F, 0x3B);
pub const EDGE: Color32 = Color32::from_rgb(0x36, 0x3C, 0x4A);
pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xE7, 0xEE);
pub const MUTED: Color32 = Color32::from_rgb(0x8C, 0x93, 0xA3);
pub const OK: Color32 = Color32::from_rgb(0x4C, 0xB8, 0x62);
pub const WARN: Color32 = Color32::from_rgb(0xE5, 0xA5, 0x3B);
pub const ERROR: Color32 = Color32::from_rgb(0xEF, 0x5B, 0x5B);

/// Tampa da tecla apagada (base e face).
pub const CAP_SKIRT: Color32 = Color32::from_rgb(0x22, 0x26, 0x30);
pub const CAP_FACE: Color32 = Color32::from_rgb(0x2D, 0x32, 0x3E);
/// Corpo do teclado no desenho.
pub const DECK: Color32 = Color32::from_rgb(0x12, 0x14, 0x1A);

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub fn with_alpha(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Luminância relativa aproximada (0..1).
pub fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Intensidade da luz de um LED (0..1), pelo canal mais forte.
pub fn intensity(c: Rgb) -> f32 {
    c.iter().copied().max().unwrap_or(0) as f32 / 255.0
}

/// Deixa uma cor de LED legível como destaque sobre o fundo escuro.
pub fn accent_from(c: Rgb) -> Color32 {
    let color = rgb(c);
    if intensity(c) < 0.05 {
        return Color32::from_rgb(0x7F, 0xB2, 0xFF);
    }
    // Normaliza o brilho (um LED a 30% vira a mesma cor a 100%) e clareia
    // um pouco as cores muito escuras, como azul puro.
    let k = 1.0 / intensity(c);
    let full = Color32::from_rgb(
        (color.r() as f32 * k).min(255.0) as u8,
        (color.g() as f32 * k).min(255.0) as u8,
        (color.b() as f32 * k).min(255.0) as u8,
    );
    let l = luminance(full);
    if l < 0.35 { mix(full, Color32::WHITE, 0.35 - l) } else { full }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes) in [
        ("chakra", &include_bytes!("../assets/fonts/ChakraPetch-Regular.ttf")[..]),
        ("chakra-semibold", &include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // As fontes padrão ficam como reserva para setas e símbolos (⌫ ↑ ←).
    let family = |first: &str| std::iter::once(first.to_string()).chain(fallbacks.iter().cloned()).collect();
    fonts.families.insert(FontFamily::Proportional, family("chakra"));
    fonts.families.insert(FontFamily::Name("semibold".into()), family("chakra-semibold"));
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Button, FontId::proportional(14.5)),
            (TextStyle::Heading, semibold(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(10.0, 10.0);
        s.button_padding = egui::vec2(14.0, 6.0);
        s.interact_size = egui::vec2(40.0, 28.0);
        s.slider_width = 240.0;
        s.combo_width = 200.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.extreme_bg_color = DECK;
        v.faint_bg_color = PANEL;
        v.override_text_color = Some(TEXT);
        v.weak_text_color = Some(MUTED);
        v.window_stroke = Stroke::new(1.0, EDGE);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, EDGE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill) in [(&mut w.inactive, RAISED), (&mut w.hovered, EDGE), (&mut w.active, EDGE), (&mut w.open, RAISED)] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(6);
            state.expansion = 0.0;
        }
        w.hovered.bg_stroke = Stroke::new(1.0, mix(EDGE, TEXT, 0.25));
    });
}

/// Atualiza a cor de destaque (chamado quando a luz do teclado muda).
pub fn set_accent(ctx: &egui::Context, accent: Color32) {
    let on_accent = if luminance(accent) > 0.55 { BG } else { TEXT };
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = accent;
        style.visuals.selection.stroke = Stroke::new(1.0, on_accent);
        style.visuals.hyperlink_color = accent;
    });
}
