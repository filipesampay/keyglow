//! Desenho do teclado na tela.
//!
//! - **Físico**: layout ABNT2 completo, posicionado pelo código da tecla.
//!   Usado quando a calibração já mapeou as teclas.
//! - **Matriz**: a grade 6x21 de LEDs do firmware. Usada na calibração e
//!   enquanto o teclado ainda não foi calibrado.

use crate::device::{Frame, Rgb, COLS, LEDS, ROWS};
use crate::effects::row_col;
use crate::input::key_name;
use crate::theme::{self, mix, with_alpha};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::collections::{BTreeMap, BTreeSet};

/// Código usado para a tecla Fn, que não chega ao PC e não tem LED conhecido.
const FN_KEY: u16 = u16::MAX;

/// Uma tecla no layout físico, em unidades de tecla (1u = uma letra).
struct KeyShape {
    code: u16,
    rect: Rect,
    /// Segunda parte da tecla (o Enter ABNT2 tem formato de L invertido).
    extra: Option<Rect>,
}

const ROW_Y: [f32; 6] = [0.0, 1.25, 2.25, 3.25, 4.25, 5.25];
const LAYOUT_W: f32 = 22.5;
const LAYOUT_H: f32 = 6.25;

/// Layout ABNT2 completo (104 + 3 teclas). `0` = espaço vazio.
fn abnt2_layout() -> Vec<KeyShape> {
    let mut keys = Vec::new();
    let mut row = |y: f32, x0: f32, items: &[(u16, f32)]| {
        let mut x = x0;
        for &(code, w) in items {
            if code != 0 {
                keys.push(KeyShape { code, rect: Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 1.0)), extra: None });
            }
            x += w;
        }
    };
    let u = |codes: &[u16]| codes.iter().map(|&c| (c, 1.0)).collect::<Vec<_>>();

    // Linha das F
    let mut r0 = vec![(1, 1.0), (0, 1.0)];
    r0.extend(u(&[59, 60, 61, 62]));
    r0.push((0, 0.5));
    r0.extend(u(&[63, 64, 65, 66]));
    r0.push((0, 0.5));
    r0.extend(u(&[67, 68, 87, 88]));
    r0.push((0, 0.25));
    r0.extend(u(&[99, 70, 119]));
    row(ROW_Y[0], 0.0, &r0);

    // Números
    let mut r1 = u(&[41, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    r1.push((14, 2.0));
    r1.push((0, 0.25));
    r1.extend(u(&[110, 102, 104]));
    r1.push((0, 0.25));
    r1.extend(u(&[69, 98, 55, 74]));
    row(ROW_Y[1], 0.0, &r1);

    // QWERTY (o Enter é tratado à parte)
    let mut r2 = vec![(15, 1.5)];
    r2.extend(u(&[16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27]));
    r2.push((0, 1.5 + 0.25));
    r2.extend(u(&[111, 107, 109]));
    r2.push((0, 0.25));
    r2.extend(u(&[71, 72, 73, 78]));
    row(ROW_Y[2], 0.0, &r2);

    // ASDF
    let mut r3 = vec![(58, 1.75)];
    r3.extend(u(&[30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 43]));
    r3.push((0, 1.25 + 3.5));
    r3.extend(u(&[75, 76, 77, 121]));
    row(ROW_Y[3], 0.0, &r3);

    // ZXCV
    let mut r4 = vec![(42, 1.25)];
    r4.extend(u(&[86, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 89]));
    r4.push((54, 1.75));
    r4.push((0, 1.25));
    r4.push((103, 1.0));
    r4.push((0, 1.25));
    r4.extend(u(&[79, 80, 81]));
    row(ROW_Y[4], 0.0, &r4);

    // Espaço
    let mut r5: Vec<(u16, f32)> = [29, 125, 56].iter().map(|&c| (c, 1.25)).collect();
    r5.push((57, 6.25));
    r5.extend([100, FN_KEY, 127, 97].iter().map(|&c| (c, 1.25)));
    r5.push((0, 0.25));
    r5.extend(u(&[105, 108, 106]));
    r5.push((0, 0.25));
    r5.push((82, 2.0));
    r5.push((83, 1.0));
    row(ROW_Y[5], 0.0, &r5);

    // Enter ABNT2 em L: parte de cima na linha QWERTY, de baixo na ASDF.
    keys.push(KeyShape {
        code: 28,
        rect: Rect::from_min_size(Pos2::new(13.75, ROW_Y[3]), Vec2::new(1.25, 1.0)),
        extra: Some(Rect::from_min_size(Pos2::new(13.5, ROW_Y[2]), Vec2::new(1.5, 1.0))),
    });
    // Teclas de altura dupla do numérico.
    for k in keys.iter_mut().filter(|k| k.code == 96) {
        k.rect.max.y += 1.0;
    }
    keys
}

/// Legenda da tecla no padrão ABNT2.
pub fn legend(code: u16) -> String {
    match code {
        FN_KEY => "Fn",
        41 => "'",
        26 => "´",
        27 => "[",
        39 => "Ç",
        40 => "~",
        43 => "]",
        86 => "\\",
        53 => ";",
        89 => "/",
        83 => ",",
        121 => ".",
        28 | 96 => "Enter",
        57 => "",
        42 | 54 => "Shift",
        _ => return key_name(code),
    }
    .to_string()
}

/// Como uma tecla deve ser destacada além da própria luz.
#[derive(Clone, Copy, PartialEq)]
pub enum Mark {
    None,
    Pressed,
    /// Posição atual da calibração.
    Target,
    /// Já calibrada (durante a calibração).
    Mapped,
}

/// Desenha uma tecla: halo de luz, base, face e legenda.
fn draw_cap(painter: &egui::Painter, rects: &[Rect], light: Rgb, text: &str, mark: Mark, unit: f32, lit: bool) {
    let color = theme::rgb(light);
    let v = if lit { theme::intensity(light) } else { 0.0 };
    let radius = CornerRadius::same((unit * 0.12).clamp(3.0, 7.0) as u8);
    let pressed = mark == Mark::Pressed;

    for r in rects {
        // Halo: a luz vazando por baixo da tecla.
        if v > 0.04 {
            painter.rect_filled(r.expand(unit * 0.10), radius, with_alpha(color, 0.10 * v));
            painter.rect_filled(r.expand(unit * 0.04), radius, with_alpha(color, 0.28 * v));
        }
        painter.rect_filled(*r, radius, mix(theme::CAP_SKIRT, color, 0.38 * v));
    }
    for r in rects {
        let inset = unit * 0.07;
        let mut face = Rect::from_min_max(r.min + Vec2::splat(inset), r.max - Vec2::new(inset, inset * 2.2));
        if pressed {
            face = face.translate(Vec2::new(0.0, inset * 1.1));
        }
        painter.rect_filled(face, radius, mix(theme::CAP_FACE, color, 0.14 * v));
        let stroke = match mark {
            Mark::Pressed => Stroke::new(1.5, theme::TEXT),
            Mark::Target => Stroke::new(2.5, theme::WARN),
            Mark::Mapped => Stroke::new(1.5, theme::OK),
            Mark::None => Stroke::new(1.0, with_alpha(Color32::WHITE, 0.04 + 0.10 * v)),
        };
        painter.rect_stroke(face, radius, stroke, StrokeKind::Inside);
    }

    if !text.is_empty() {
        // Legenda iluminada por dentro, como nas teclas translúcidas.
        let legend_color = if v > 0.08 { mix(color, Color32::WHITE, 0.45) } else { theme::MUTED };
        let base = rects[0];
        let size = if text.chars().count() > 2 { unit * 0.22 } else { unit * 0.30 };
        let mut pos = Pos2::new(base.center().x, base.min.y + base.height() * 0.44);
        if pressed {
            pos.y += unit * 0.07;
        }
        painter.text(pos, Align2::CENTER_CENTER, text, FontId::proportional(size.clamp(9.0, 18.0)), legend_color);
    }
}

/// Luz que se espalha na "mesa" em volta do teclado, com a cor média dos LEDs.
fn draw_underglow(painter: &egui::Painter, deck: Rect, leds: impl Iterator<Item = Rgb>) {
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for c in leds {
        for i in 0..3 {
            sum[i] += c[i] as f32;
        }
        n += 1.0;
    }
    if n == 0.0 {
        return;
    }
    let avg = sum.map(|s| (s / n) as u8);
    let v = theme::intensity(avg);
    if v < 0.03 {
        return;
    }
    let color = theme::accent_from(avg);
    for i in (1..=10).rev() {
        let grow = i as f32 * 7.0;
        let alpha = 0.035 * v * (1.0 - i as f32 / 11.0);
        painter.rect_filled(deck.expand(grow), CornerRadius::same((14.0 + grow).min(127.0) as u8), with_alpha(color, alpha));
    }
}

pub struct ViewData<'a> {
    pub preview: &'a Frame,
    pub key_to_led: &'a BTreeMap<u16, usize>,
    pub pressed: &'a BTreeSet<u16>,
    /// Posição atual da calibração, se ela estiver em andamento.
    pub calibrating: Option<usize>,
}

/// Resultado da interação com o desenho.
pub struct ViewResponse {
    /// LED sob o ponteiro quando houve clique ou arraste.
    pub led: Option<usize>,
    pub clicked: bool,
    pub dragged: bool,
}

/// Desenho físico (ABNT2). Teclas não calibradas aparecem apagadas.
pub fn physical(ui: &mut egui::Ui, data: &ViewData, max_height: f32) -> ViewResponse {
    let layout = abnt2_layout();
    let pad = 0.45;
    let unit = ((ui.available_width() - 40.0) / (LAYOUT_W + pad * 2.0))
        .min(max_height / (LAYOUT_H + pad * 2.0))
        .clamp(18.0, 64.0);
    let size = Vec2::new((LAYOUT_W + pad * 2.0) * unit, (LAYOUT_H + pad * 2.0) * unit);
    let (outer, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), size.y + 30.0), Sense::click_and_drag());
    let deck = Rect::from_center_size(outer.center(), size);
    let painter = ui.painter_at(outer);

    let origin = deck.min + Vec2::splat(pad * unit);
    let to_screen = |r: Rect| Rect::from_min_max(origin + r.min.to_vec2() * unit, origin + r.max.to_vec2() * unit).shrink(unit * 0.035);

    draw_underglow(&painter, deck, data.key_to_led.values().map(|&led| data.preview[led]));
    painter.rect_filled(deck, CornerRadius::same((unit * 0.3) as u8), theme::DECK);
    painter.rect_stroke(deck, CornerRadius::same((unit * 0.3) as u8), Stroke::new(1.0, theme::EDGE), StrokeKind::Inside);

    let pointer = response.interact_pointer_pos();
    let mut hit = None;
    for key in &layout {
        let mut rects = vec![to_screen(key.rect)];
        if let Some(extra) = key.extra {
            rects.push(to_screen(extra));
        }
        let led = data.key_to_led.get(&key.code).copied();
        let light = led.map(|l| data.preview[l]).unwrap_or([0; 3]);
        let mark = if data.pressed.contains(&key.code) { Mark::Pressed } else { Mark::None };
        draw_cap(&painter, &rects, light, &legend(key.code), mark, unit, led.is_some());
        if let (Some(p), Some(l)) = (pointer, led)
            && rects.iter().any(|r| r.contains(p))
        {
            hit = Some(l);
        }
    }

    if hit.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ViewResponse { led: hit, clicked: response.clicked(), dragged: response.dragged() }
}

/// Grade 6x21 do firmware, usada na calibração.
pub fn matrix(ui: &mut egui::Ui, data: &ViewData, max_height: f32) -> ViewResponse {
    let led_to_key: BTreeMap<usize, u16> = data.key_to_led.iter().map(|(&k, &l)| (l, k)).collect();
    let unit = ((ui.available_width() - 40.0) / COLS as f32).min(max_height / (ROWS as f32 + 0.6)).clamp(18.0, 60.0);
    let size = Vec2::new(unit * COLS as f32, unit * ROWS as f32);
    let (outer, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), size.y + unit * 0.6), Sense::click_and_drag());
    let grid = Rect::from_center_size(outer.center(), size);
    let painter = ui.painter_at(outer);
    painter.rect_filled(grid.expand(unit * 0.25), CornerRadius::same(10), theme::DECK);

    let mut hit = None;
    for led in 0..LEDS {
        let (row, col) = row_col(led);
        let rect = Rect::from_min_size(grid.min + Vec2::new(col as f32, row as f32) * unit, Vec2::splat(unit)).shrink(unit * 0.05);
        let code = led_to_key.get(&led).copied();
        let mark = if data.calibrating == Some(led) {
            Mark::Target
        } else if code.is_some_and(|c| data.pressed.contains(&c)) {
            Mark::Pressed
        } else if data.calibrating.is_some() && code.is_some() {
            Mark::Mapped
        } else {
            Mark::None
        };
        let text = code.map(legend).unwrap_or_default();
        draw_cap(&painter, &[rect], data.preview[led], &text, mark, unit, true);
        if response.interact_pointer_pos().is_some_and(|p| rect.contains(p)) {
            hit = Some(led);
        }
    }
    if hit.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ViewResponse { led: hit, clicked: response.clicked(), dragged: response.dragged() }
}
