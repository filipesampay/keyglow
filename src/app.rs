//! Interface gráfica (egui).

use crate::config::Config;
use crate::device::{hw_mode_name, HwMode, Rgb, COLS, HW_MODES, LEDS, ROWS};
use crate::effects::{row_col, EFFECTS};
use crate::engine::{Cmd, Shared, Status};
use crate::input::{key_name, InputState};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct App {
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    tx: Sender<Cmd>,
    engine: Option<JoinHandle<()>>,
    config: Config,
    /// Efeito de fábrica sendo editado (só vai ao teclado ao clicar em Aplicar).
    hw_edit: HwMode,
    synced_from_device: bool,
    brush: Rgb,
    /// LED atual da calibração, se ela estiver em andamento.
    calibrating: Option<usize>,
    /// Última tecla mapeada na calibração (código, LED), para conferência.
    last_mapped: Option<(u16, usize)>,
}

impl App {
    pub fn new(
        shared: Arc<Mutex<Shared>>,
        input: Arc<Mutex<InputState>>,
        tx: Sender<Cmd>,
        engine: JoinHandle<()>,
        mut config: Config,
    ) -> Self {
        // Limpa mapeamentos inválidos de versões anteriores (volume, tecla fantasma).
        config.key_to_led.retain(|&code, _| is_mappable(code));
        shared.lock().unwrap().key_to_led = config.key_to_led.clone();
        Self {
            shared,
            input,
            tx,
            engine: Some(engine),
            hw_edit: config.hw,
            config,
            synced_from_device: false,
            brush: [255, 255, 255],
            calibrating: None,
            last_mapped: None,
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    fn set_calibration(&mut self, led: Option<usize>) {
        self.calibrating = led;
        self.shared.lock().unwrap().calibrate_led = led;
    }

    /// Avança a calibração pulando LEDs que não existem no fim.
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

fn to_color32(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
        self.handle_key_events();

        let (status, fps, preview, device_hw) = {
            let sh = self.shared.lock().unwrap();
            (sh.status.clone(), sh.fps, sh.preview, sh.device_hw)
        };
        if !self.synced_from_device
            && let Some(hw) = device_hw {
                self.hw_edit = hw;
                self.config.hw = hw;
                self.synced_from_device = true;
            }
        let (pressed, input_error, input_devices) = {
            let inp = self.input.lock().unwrap();
            (inp.pressed.clone(), inp.error.clone(), inp.devices)
        };

        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("keyglow");
                ui.separator();
                match &status {
                    Status::Connected => {
                        ui.colored_label(Color32::LIGHT_GREEN, "● Teclado conectado");
                        if self.config.software_effect.is_some() || self.calibrating.is_some() {
                            ui.label(format!("{fps:.0} quadros/s"));
                        }
                    }
                    Status::Connecting(msg) => {
                        ui.colored_label(Color32::LIGHT_RED, format!("● {msg}"));
                    }
                }
                ui.separator();
                match &input_error {
                    Some(err) if input_devices == 0 => {
                        ui.colored_label(Color32::YELLOW, format!("Teclas: {err}"));
                    }
                    _ => {
                        ui.label(format!("Teclas: lendo {input_devices} dispositivo(s)"));
                    }
                }
            });
        });

        egui::Panel::left("controls").default_size(310.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(led) = self.calibrating {
                self.calibration_panel(ui, led);
                ui.separator();
            }
            self.keyboard_view(ui, &preview, &pressed);
            ui.add_space(8.0);
            let names: Vec<String> = pressed.iter().map(|&c| key_name(c)).collect();
            ui.horizontal_wrapped(|ui| {
                ui.strong("Teclas pressionadas:");
                if names.is_empty() {
                    ui.weak("nenhuma");
                } else {
                    for n in names {
                        ui.code(n);
                    }
                }
            });
        });
    }
}

impl App {
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Modo");
        let mut software = self.config.software_effect.is_some();
        let before = software;
        ui.radio_value(&mut software, false, "Efeitos do teclado (de fábrica)");
        ui.radio_value(&mut software, true, "Efeitos do app (tempo real)");
        if software != before {
            if software {
                let i = self.config.software_effect.unwrap_or(0);
                self.config.software_effect = Some(i);
                self.send(Cmd::Software(i));
            } else {
                self.config.software_effect = None;
                self.send(Cmd::Hardware(self.hw_edit));
            }
        }
        ui.separator();

        if software {
            self.software_controls(ui);
        } else {
            self.hardware_controls(ui);
        }

        ui.separator();
        ui.heading("Calibração das teclas");
        ui.label(format!(
            "{} teclas mapeadas. Calibre para os efeitos que reagem ao digitar e para os nomes na tela.",
            self.config.key_to_led.len()
        ));
        ui.horizontal(|ui| {
            if self.calibrating.is_none() && ui.button("Calibrar teclas").clicked() {
                self.set_calibration(Some(0));
            }
            if !self.config.key_to_led.is_empty() && ui.button("Apagar calibração").clicked() {
                self.config.key_to_led.clear();
                self.shared.lock().unwrap().key_to_led.clear();
                self.config.save();
            }
        });
    }

    fn hardware_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Gravados no teclado: continuam funcionando com o app fechado.");
        let hw = &mut self.hw_edit;
        egui::ComboBox::from_label("Efeito")
            .selected_text(hw_mode_name(hw.mode))
            .show_ui(ui, |ui| {
                for &(value, name) in HW_MODES {
                    ui.selectable_value(&mut hw.mode, value, name);
                }
            });
        ui.horizontal(|ui| {
            ui.label("Cor");
            ui.color_edit_button_srgb(&mut hw.color);
            ui.checkbox(&mut hw.random, "Cores aleatórias");
        });
        ui.add(egui::Slider::new(&mut hw.brightness, 0..=4).text("Brilho"));
        // No firmware, 0 é o mais rápido; na tela, mais alto = mais rápido.
        let mut speed = 5 - hw.speed.min(5);
        ui.add(egui::Slider::new(&mut speed, 0..=5).text("Velocidade"));
        hw.speed = 5 - speed;
        let mut reverse = hw.direction != 0;
        ui.checkbox(&mut reverse, "Inverter direção");
        hw.direction = reverse as u8;

        let changed = self.hw_edit != self.config.hw;
        if ui.add_enabled(changed, egui::Button::new("Aplicar no teclado")).clicked() {
            self.config.hw = self.hw_edit;
            self.send(Cmd::Hardware(self.hw_edit));
            self.config.save();
        }
    }

    fn software_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Calculados pelo app: só funcionam com ele aberto.");
        let current = self.config.software_effect.unwrap_or(0);
        for (i, effect) in EFFECTS.iter().enumerate() {
            if ui.selectable_label(i == current, effect.name).on_hover_text(effect.description).clicked() && i != current {
                self.config.software_effect = Some(i);
                self.send(Cmd::Software(i));
            }
        }
        if let Some(effect) = EFFECTS.get(current) {
            ui.weak(effect.description);
        }
        ui.separator();

        let p = &mut self.config.params;
        ui.horizontal(|ui| {
            ui.label("Cor principal");
            ui.color_edit_button_srgb(&mut p.primary);
        });
        ui.horizontal(|ui| {
            ui.label("Cor de fundo");
            ui.color_edit_button_srgb(&mut p.secondary);
        });
        ui.add(egui::Slider::new(&mut p.speed, 0.1..=3.0).text("Velocidade"));
        ui.add(egui::Slider::new(&mut p.brightness, 0.0..=1.0).text("Brilho"));

        if EFFECTS.get(current).is_some_and(|e| e.name == "Pintura") {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Pincel");
                ui.color_edit_button_srgb(&mut self.brush);
                if ui.button("Pintar tudo").clicked() {
                    p.paint = vec![self.brush; LEDS];
                }
            });
            ui.weak("Clique ou arraste sobre as teclas da tela.");
        }
        self.shared.lock().unwrap().params = self.config.params.clone();
    }

    fn calibration_panel(&mut self, ui: &mut egui::Ui, led: usize) {
        let (row, col) = row_col(led);
        ui.heading(format!("Calibrando: LED {} de {LEDS} (linha {}, coluna {})", led + 1, row + 1, col + 1));
        ui.label("Aperte a tecla que está piscando em branco no teclado. As já calibradas ficam verdes.");
        ui.label("Nada piscando? Essa posição não tem LED: gire o volume para cima para pular (para baixo volta).");
        ui.weak("Dica: clique em uma posição da tela para ir direto até ela.");
        if let Some((code, l)) = self.last_mapped {
            let (r, c) = row_col(l);
            ui.label(format!("Última: {} → linha {}, coluna {}", key_name(code), r + 1, c + 1));
        }
        ui.horizontal(|ui| {
            if ui.button("Pular (sem tecla)").clicked() {
                self.calibration_next(led);
            }
            if ui.button("Pular resto da linha").clicked() {
                let next_row_start = (row + 1) * COLS;
                if next_row_start >= LEDS {
                    self.finish_calibration();
                } else {
                    self.set_calibration(Some(next_row_start));
                }
            }
            if led > 0 && ui.button("Voltar").clicked() {
                self.set_calibration(Some(led - 1));
            }
            if ui.button("Concluir").clicked() {
                self.finish_calibration();
            }
        });
    }

    /// Desenha a matriz 6x21 com as cores atuais e as teclas pressionadas.
    fn keyboard_view(&mut self, ui: &mut egui::Ui, preview: &[Rgb; LEDS], pressed: &std::collections::BTreeSet<u16>) {
        let led_to_key = self.config.led_to_key();
        let width = ui.available_width();
        let cell = (width / COLS as f32).clamp(24.0, 60.0);
        let size = Vec2::new(cell * COLS as f32, cell * ROWS as f32);
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let origin = response.rect.min;
        let pressed_leds: Vec<usize> = pressed.iter().filter_map(|c| self.config.key_to_led.get(c).copied()).collect();

        for led in 0..LEDS {
            let (row, col) = row_col(led);
            let rect = Rect::from_min_size(
                origin + Vec2::new(col as f32 * cell, row as f32 * cell),
                Vec2::splat(cell),
            )
            .shrink(2.0);
            let color = preview[led];
            painter.rect_filled(rect, 4.0, to_color32(color));

            let is_pressed = pressed_leds.contains(&led);
            let stroke = if self.calibrating == Some(led) {
                Stroke::new(3.0, Color32::YELLOW)
            } else if self.calibrating.is_some() && led_to_key.contains_key(&led) {
                Stroke::new(2.0, Color32::LIGHT_GREEN)
            } else if is_pressed {
                Stroke::new(3.0, Color32::WHITE)
            } else {
                Stroke::new(1.0, Color32::from_gray(60))
            };
            painter.rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);

            if let Some(&code) = led_to_key.get(&led) {
                let luma = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
                let text = if luma > 140.0 { Color32::BLACK } else { Color32::WHITE };
                painter.text(rect.center(), Align2::CENTER_CENTER, key_name(code), FontId::proportional(cell * 0.28), text);
            }
        }

        let Some(led) = response.interact_pointer_pos().and_then(|pos| led_at(origin, cell, pos)) else {
            return;
        };
        let painting = self.config.software_effect.and_then(|i| EFFECTS.get(i)).is_some_and(|e| e.name == "Pintura");
        if self.calibrating.is_some() {
            // Na calibração, clicar leva direto àquela posição.
            if response.clicked() {
                self.set_calibration(Some(led));
            }
        } else if painting {
            if response.clicked() || response.dragged() {
                self.config.params.paint[led] = self.brush;
                self.shared.lock().unwrap().params = self.config.params.clone();
            }
        } else if response.clicked() {
            // Fora da pintura: clicar numa posição recalibra a partir dela.
            self.set_calibration(Some(led));
        }
    }
}

fn led_at(origin: Pos2, cell: f32, pos: Pos2) -> Option<usize> {
    let d = pos - origin;
    if d.x < 0.0 || d.y < 0.0 {
        return None;
    }
    let (col, row) = ((d.x / cell) as usize, (d.y / cell) as usize);
    (col < COLS && row < ROWS).then_some(row * COLS + col)
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
