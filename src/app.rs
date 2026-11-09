//! Interface gráfica (egui).
//!
//! Estrutura da janela:
//! - topo: nome, seletor "Do teclado | Do app" e status da conexão;
//! - centro: o teclado desenhado, com a luz atual e as teclas pressionadas;
//! - base: lista de efeitos à esquerda e ajustes do efeito à direita.

use crate::config::Config;
use crate::device::{hw_mode_name, HwMode, Rgb, COLS, HW_MODES, LEDS};
use crate::effects::{row_col, Control, EFFECTS};
use crate::engine::{Cmd, Shared, Status};
use crate::input::InputState;
use crate::keyboard_view::{self, legend, ViewData};
use crate::theme::{self, semibold};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Vec2};
use std::collections::BTreeSet;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Mínimo de teclas calibradas para desenhar o teclado físico.
const MIN_MAPPED_FOR_PHYSICAL: usize = 20;
/// Intervalo mínimo entre gravações de efeito de fábrica (cada uma escreve
/// na memória do teclado).
const HW_APPLY_INTERVAL: Duration = Duration::from_millis(250);
const CONTROLS_HEIGHT: f32 = 300.0;

pub struct App {
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    tx: Sender<Cmd>,
    engine: Option<JoinHandle<()>>,
    config: Config,
    /// Efeito de fábrica sendo editado; vai ao teclado com um pequeno atraso.
    hw_edit: HwMode,
    last_hw_apply: Instant,
    synced_from_device: bool,
    brush: Rgb,
    /// LED atual da calibração, se ela estiver em andamento.
    calibrating: Option<usize>,
    /// Última tecla mapeada na calibração (código, LED), para conferência.
    last_mapped: Option<(u16, usize)>,
    accent: Color32,
    screenshot: Option<crate::screenshot::Screenshot>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Arc<Mutex<Shared>>,
        input: Arc<Mutex<InputState>>,
        tx: Sender<Cmd>,
        engine: JoinHandle<()>,
        mut config: Config,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        // Limpa mapeamentos inválidos de versões anteriores (volume, tecla fantasma).
        config.key_to_led.retain(|&code, _| is_mappable(code));
        shared.lock().unwrap().key_to_led = config.key_to_led.clone();
        Self {
            shared,
            input,
            tx,
            engine: Some(engine),
            hw_edit: config.hw,
            last_hw_apply: Instant::now(),
            config,
            synced_from_device: false,
            brush: [255, 255, 255],
            calibrating: None,
            last_mapped: None,
            accent: Color32::TRANSPARENT,
            screenshot: crate::screenshot::Screenshot::from_env(),
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    fn is_software(&self) -> bool {
        self.config.software_effect.is_some()
    }

    fn set_mode(&mut self, software: bool) {
        if software == self.is_software() {
            return;
        }
        if software {
            let i = self.config.software_effect.unwrap_or(0);
            self.config.software_effect = Some(i);
            self.send(Cmd::Software(i));
        } else {
            self.config.software_effect = None;
            self.send(Cmd::Hardware(self.hw_edit));
        }
    }

    /// Grava o efeito de fábrica editado, no máximo a cada `HW_APPLY_INTERVAL`.
    fn apply_hw_if_changed(&mut self) {
        if self.is_software() || self.hw_edit == self.config.hw || self.last_hw_apply.elapsed() < HW_APPLY_INTERVAL {
            return;
        }
        self.config.hw = self.hw_edit;
        self.last_hw_apply = Instant::now();
        self.send(Cmd::Hardware(self.hw_edit));
        self.config.save();
    }

    /// Cor que o teclado está mostrando, usada como destaque da interface.
    fn light_color(&self) -> Rgb {
        if self.is_software() {
            self.config.params.primary
        } else if self.hw_edit.random {
            [127, 178, 255]
        } else {
            self.hw_edit.color
        }
    }

    fn set_calibration(&mut self, led: Option<usize>) {
        self.calibrating = led;
        self.shared.lock().unwrap().calibrate_led = led;
    }

    fn calibration_next(&mut self, from: usize) {
        if from + 1 >= LEDS {
            self.finish_calibration();
        } else {
            self.set_calibration(Some(from + 1));
        }
    }

    fn finish_calibration(&mut self) {
        self.set_calibration(None);
        self.config.save();
    }

    fn handle_key_events(&mut self) {
        let events = std::mem::take(&mut self.input.lock().unwrap().gui_events);
        for (code, down) in events {
            let Some(led) = self.calibrating else { return };
            if !down {
                continue;
            }
            // O botão de volume navega na calibração: muitas posições da
            // matriz não têm LED, então não há nada para apertar nelas.
            match code {
                KEY_VOLUMEUP => self.calibration_next(led),
                KEY_VOLUMEDOWN => self.set_calibration(Some(led.saturating_sub(1))),
                _ if !is_mappable(code) => {}
                _ => {
                    // A tecla apertada vira a dona do LED atual.
                    self.config.key_to_led.retain(|&k, &mut l| k != code && l != led);
                    self.config.key_to_led.insert(code, led);
                    self.shared.lock().unwrap().key_to_led = self.config.key_to_led.clone();
                    self.last_mapped = Some((code, led));
                    self.calibration_next(led);
                }
            }
        }
    }
}

const KEY_MUTE: u16 = 113;
const KEY_VOLUMEDOWN: u16 = 114;
const KEY_VOLUMEUP: u16 = 115;

/// Teclas que podem receber um LED. O botão de volume não tem LED próprio na matriz.
pub fn is_mappable(code: u16) -> bool {
    !matches!(code, KEY_MUTE | KEY_VOLUMEDOWN | KEY_VOLUMEUP) && code != crate::input::KEY_PHANTOM
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(33));
        if let Some(shot) = self.screenshot.as_mut() {
            shot.tick(&ctx);
        }
        self.handle_key_events();

        let (status, fps, preview, device_hw) = {
            let sh = self.shared.lock().unwrap();
            (sh.status.clone(), sh.fps, sh.preview, sh.device_hw)
        };
        if !self.synced_from_device
            && let Some(hw) = device_hw
        {
            self.hw_edit = hw;
            self.config.hw = hw;
            self.synced_from_device = true;
        }
        let (pressed, input_error, input_devices) = {
            let inp = self.input.lock().unwrap();
            (inp.pressed.clone(), inp.error.clone(), inp.devices)
        };

        let accent = theme::accent_from(self.light_color());
        if accent != self.accent {
            self.accent = accent;
            theme::set_accent(&ctx, accent);
        }

        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(24, 14)))
            .show(ui, |ui| self.header(ui, &status, fps));

        egui::Panel::bottom("controls")
            .resizable(false)
            .exact_size(CONTROLS_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 16 })
                    .stroke(Stroke::new(1.0, theme::EDGE)),
            )
            .show(ui, |ui| self.controls(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(24, 8)))
            .show(ui, |ui| self.hero(ui, &preview, &pressed, input_error.filter(|_| input_devices == 0)));

        self.apply_hw_if_changed();
    }
}

impl App {
    fn header(&mut self, ui: &mut egui::Ui, status: &Status, fps: f32) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("keyglow").font(semibold(24.0)).color(theme::TEXT));
            ui.add_space(28.0);

            let mut software = self.is_software();
            segmented(ui, &mut software, [(false, "Do teclado"), (true, "Do app")]);
            self.set_mode(software);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match status {
                Status::Connected => {
                    if self.is_software() || self.calibrating.is_some() {
                        ui.label(RichText::new(format!("{fps:.0} quadros/s")).small().weak());
                    }
                    ui.label("Teclado conectado");
                    status_dot(ui, theme::OK);
                }
                Status::Connecting(msg) => {
                    ui.label(RichText::new(msg).color(theme::ERROR));
                    status_dot(ui, theme::ERROR);
                }
            });
        });
    }

    fn hero(&mut self, ui: &mut egui::Ui, preview: &[Rgb; LEDS], pressed: &BTreeSet<u16>, input_error: Option<String>) {
        if let Some(led) = self.calibrating {
            self.calibration_banner(ui, led);
        }

        let mapped = self.config.key_to_led.len();
        let data = ViewData { preview, key_to_led: &self.config.key_to_led, pressed, calibrating: self.calibrating };
        let max_h = ui.available_height() - 44.0;
        let view = if self.calibrating.is_none() && mapped >= MIN_MAPPED_FOR_PHYSICAL {
            keyboard_view::physical(ui, &data, max_h)
        } else {
            keyboard_view::matrix(ui, &data, max_h)
        };
        self.handle_view_click(view);

        // Rodapé do teclado: teclas pressionadas e calibração.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Pressionadas").weak());
            let keys: Vec<u16> = pressed.iter().copied().filter(|&c| is_mappable(c)).collect();
            if keys.is_empty() {
                ui.label(RichText::new("nenhuma tecla").weak());
            }
            for code in keys {
                key_chip(ui, &legend(code), self.accent);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.calibrating.is_some() {
                    return;
                }
                let label = if mapped == 0 { "Calibrar teclas" } else { "Recalibrar" };
                if ui.button(label).clicked() {
                    self.set_calibration(Some(0));
                }
                match &input_error {
                    Some(err) => {
                        ui.label(RichText::new(err).color(theme::WARN));
                    }
                    None if mapped == 0 => {
                        ui.label(RichText::new("Calibre para ver o teclado de verdade e usar os efeitos que reagem ao digitar").weak());
                    }
                    None => {
                        ui.label(RichText::new(format!("{mapped} teclas calibradas")).weak());
                    }
                }
            });
        });
    }

    fn handle_view_click(&mut self, view: keyboard_view::ViewResponse) {
        let Some(led) = view.led else { return };
        if self.calibrating.is_some() {
            if view.clicked {
                self.set_calibration(Some(led));
            }
        } else if self.is_painting() && (view.clicked || view.dragged) {
            self.config.params.paint[led] = self.brush;
            self.shared.lock().unwrap().params = self.config.params.clone();
        }
    }

    fn is_painting(&self) -> bool {
        self.config.software_effect.and_then(|i| EFFECTS.get(i)).is_some_and(|e| e.name == "Pintura")
    }

    fn calibration_banner(&mut self, ui: &mut egui::Ui, led: usize) {
        let (row, col) = row_col(led);
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::WARN))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("Calibrando a posição {} de {LEDS}", led + 1)).font(semibold(17.0)));
                        ui.label(format!(
                            "Aperte a tecla que está piscando no teclado (linha {}, coluna {}). Se nada piscar, gire o volume para cima para pular.",
                            row + 1,
                            col + 1
                        ));
                        let last = match self.last_mapped {
                            Some((code, l)) => {
                                let (r, c) = row_col(l);
                                format!("Última calibrada: {} na linha {}, coluna {}", legend(code), r + 1, c + 1)
                            }
                            None => "Volume para baixo volta uma posição. Clique numa posição da grade para ir direto até ela.".into(),
                        };
                        ui.label(RichText::new(last).weak());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Concluir").clicked() {
                            self.finish_calibration();
                        }
                        if ui.button("Pular linha").clicked() {
                            let next = (row + 1) * COLS;
                            if next >= LEDS { self.finish_calibration() } else { self.set_calibration(Some(next)) }
                        }
                        if ui.button("Pular").clicked() {
                            self.calibration_next(led);
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            // Grade de efeitos: cabe inteira, sem rolagem.
            ui.vertical(|ui| {
                ui.label(RichText::new("Efeitos").font(semibold(16.0)));
                ui.add_space(2.0);
                let accent = self.accent;
                if self.is_software() {
                    let current = self.config.software_effect.unwrap_or(0);
                    let names: Vec<&str> = EFFECTS.iter().map(|e| e.name).collect();
                    if let Some(i) = effect_grid(ui, "sw_effects", &names, Some(current), 2, accent)
                        && i != current
                    {
                        self.config.software_effect = Some(i);
                        self.send(Cmd::Software(i));
                    }
                } else {
                    let names: Vec<&str> = HW_MODES.iter().map(|(_, n)| *n).collect();
                    let current = HW_MODES.iter().position(|(m, _)| *m == self.hw_edit.mode);
                    if let Some(i) = effect_grid(ui, "hw_effects", &names, current, 3, accent) {
                        self.hw_edit.mode = HW_MODES[i].0;
                    }
                }
            });

            ui.add_space(20.0);
            ui.separator();
            ui.add_space(20.0);

            ui.vertical(|ui| {
                if self.is_software() {
                    self.software_settings(ui);
                } else {
                    self.hardware_settings(ui);
                }
            });
        });
    }

    fn hardware_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(hw_mode_name(self.hw_edit.mode)).font(semibold(20.0)));
        ui.label(RichText::new("Fica gravado no teclado e continua funcionando com o keyglow fechado.").weak());
        ui.add_space(8.0);

        let hw = &mut self.hw_edit;
        egui::Grid::new("hw_settings").num_columns(2).spacing([28.0, 14.0]).show(ui, |ui| {
            ui.label(RichText::new("Cor").weak());
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!hw.random, |ui| color_button(ui, &mut hw.color));
                ui.checkbox(&mut hw.random, "Cores aleatórias");
            });
            ui.end_row();

            ui.label(RichText::new("Brilho").weak());
            ui.add(egui::Slider::new(&mut hw.brightness, 0..=4).show_value(false));
            ui.end_row();

            // No firmware, 0 é o mais rápido; na tela, para a direita = mais rápido.
            ui.label(RichText::new("Velocidade").weak());
            let mut speed = 5 - hw.speed.min(5);
            ui.add(egui::Slider::new(&mut speed, 0..=5).show_value(false));
            hw.speed = 5 - speed;
            ui.end_row();

            ui.label(RichText::new("Direção").weak());
            let mut reverse = hw.direction != 0;
            segmented(ui, &mut reverse, [(false, "Normal"), (true, "Invertida")]);
            hw.direction = reverse as u8;
            ui.end_row();
        });
    }

    fn software_settings(&mut self, ui: &mut egui::Ui) {
        let Some(effect) = self.config.software_effect.and_then(|i| EFFECTS.get(i)) else { return };
        ui.label(RichText::new(effect.name).font(semibold(20.0)));
        ui.label(RichText::new(effect.description).weak());
        ui.add_space(8.0);

        let painting = self.is_painting();
        let p = &mut self.config.params;
        egui::Grid::new("sw_settings").num_columns(2).spacing([28.0, 14.0]).show(ui, |ui| {
            if painting {
                ui.label(RichText::new("Pincel").weak());
                ui.horizontal(|ui| {
                    color_button(ui, &mut self.brush);
                    if ui.button("Pintar todas as teclas").clicked() {
                        p.paint = vec![self.brush; LEDS];
                    }
                });
                ui.end_row();
            }
            for control in effect.controls {
                match *control {
                    Control::Primary(label) => {
                        ui.label(RichText::new(label).weak());
                        color_button(ui, &mut p.primary);
                    }
                    Control::Secondary(label) => {
                        ui.label(RichText::new(label).weak());
                        color_button(ui, &mut p.secondary);
                    }
                    Control::Speed => {
                        ui.label(RichText::new("Velocidade").weak());
                        ui.add(egui::Slider::new(&mut p.speed, 0.1..=3.0).custom_formatter(|v, _| format!("{v:.1}×")));
                    }
                }
                ui.end_row();
            }

            ui.label(RichText::new("Brilho").weak());
            ui.add(egui::Slider::new(&mut p.brightness, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            ui.end_row();
        });
        if painting {
            ui.label(RichText::new("Clique ou arraste sobre as teclas para pintar.").weak());
        }
        self.shared.lock().unwrap().params = self.config.params.clone();
    }
}

// ---------------------------------------------------------------------------
// Componentes

/// Controle segmentado de duas opções.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: [(T, &str); 2]) {
    egui::Frame::new()
        .fill(theme::DECK)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label) in options {
                    let selected = *value == option;
                    let text = RichText::new(label).color(if selected { theme::TEXT } else { theme::MUTED });
                    let button = egui::Button::new(text)
                        .fill(if selected { theme::RAISED } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(6))
                        .min_size(Vec2::new(104.0, 28.0));
                    if ui.add(button).clicked() {
                        *value = option;
                    }
                }
            });
        });
}

/// Grade de efeitos em colunas; devolve o índice clicado.
fn effect_grid(ui: &mut egui::Ui, id: &str, names: &[&str], selected: Option<usize>, columns: usize, accent: Color32) -> Option<usize> {
    let mut clicked = None;
    egui::Grid::new(id).num_columns(columns).spacing([4.0, 2.0]).show(ui, |ui| {
        for (i, name) in names.iter().enumerate() {
            if effect_row(ui, selected == Some(i), name, accent).clicked() {
                clicked = Some(i);
            }
            if (i + 1) % columns == 0 {
                ui.end_row();
            }
        }
    });
    clicked
}

/// Item da grade de efeitos; o selecionado ganha uma barra com a cor da luz.
fn effect_row(ui: &mut egui::Ui, selected: bool, name: &str, accent: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(176.0, 32.0), Sense::click());
    let fill = if selected {
        theme::RAISED
    } else if response.hovered() {
        theme::mix(theme::PANEL, theme::RAISED, 0.5)
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if selected {
        let bar = egui::Rect::from_min_size(rect.min + Vec2::new(0.0, 7.0), Vec2::new(3.0, rect.height() - 14.0));
        painter.rect_filled(bar, CornerRadius::same(2), accent);
    }
    painter.text(
        rect.left_center() + Vec2::new(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(14.5),
        if selected { theme::TEXT } else { theme::MUTED },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn key_chip(ui: &mut egui::Ui, text: &str, accent: Color32) {
    let text = if text.is_empty() { "Espaço" } else { text };
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(Stroke::new(1.0, theme::with_alpha(accent, 0.6)))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).font(semibold(13.0))));
}

/// Botão de cor com borda, para cores claras não sumirem no fundo.
fn color_button(ui: &mut egui::Ui, color: &mut Rgb) -> egui::Response {
    let response = ui.color_edit_button_srgb(color);
    ui.painter().rect_stroke(response.rect, CornerRadius::same(4), Stroke::new(1.0, theme::EDGE), egui::StrokeKind::Outside);
    response
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

impl Drop for App {
    /// Ao fechar: salva a configuração e devolve o teclado ao efeito de fábrica.
    fn drop(&mut self) {
        self.config.save();
        self.send(Cmd::Shutdown);
        if let Some(handle) = self.engine.take() {
            let _ = handle.join();
        }
    }
}
