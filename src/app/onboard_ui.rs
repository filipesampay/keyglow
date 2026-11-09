//! Telas do que fica gravado no teclado: o que cada tecla faz, macros e
//! informações/padrão de fábrica.

use super::App;
use crate::engine::{Cmd, OnboardCmd, OnboardState};
use crate::keyboard_view::legend;
use crate::keymap::{evdev_to_hid_usage, hid_usage_to_evdev, keyboard_usages};
use crate::onboard::{
    self, Cell, KeyAction, Macro, MacroInput, MacroRun, MacroStep, MAX_MACROS, MAX_MACRO_STEPS, MEDIA_KEYS, MODIFIERS, MOUSE_BUTTONS,
    POWER_ACTIONS,
};
use crate::theme::{self, semibold};
use eframe::egui::{self, RichText};
use std::collections::BTreeSet;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Light,
    Keys,
    Macros,
    Keyboard,
}

/// Estado das telas de teclas e macros.
pub struct OnboardUi {
    pub page: Page,
    /// (posição na tabela, LED) da tecla escolhida.
    pub selected: Option<(usize, usize)>,
    /// Função sendo montada para a tecla escolhida.
    draft: KeyAction,
    /// A próxima tecla apertada vira a tecla do rascunho.
    capture: bool,
    keys_requested: bool,
    macros_requested: bool,
    /// Macros sendo editadas (sempre [`MAX_MACROS`] posições).
    macros: Vec<Macro>,
    /// Últimas macros lidas do teclado, já com as [`MAX_MACROS`] posições.
    loaded_macros: Option<Vec<Macro>>,
    macro_sel: usize,
    /// Gravando passos a partir do teclado; guarda a hora do último passo.
    recording: Option<Instant>,
    new_key: u8,
    new_mouse: u8,
    default_delay: u16,
    confirm_reset: bool,
    pub state: OnboardState,
}

impl Default for OnboardUi {
    fn default() -> Self {
        Self {
            page: Page::Light,
            selected: None,
            draft: KeyAction::Disabled,
            capture: false,
            keys_requested: false,
            macros_requested: false,
            macros: vec![Vec::new(); MAX_MACROS],
            loaded_macros: None,
            macro_sel: 0,
            recording: None,
            new_key: 0x04,
            new_mouse: 1,
            default_delay: 20,
            confirm_reset: false,
            state: OnboardState::default(),
        }
    }
}

impl OnboardUi {
    /// Teclado novo (ou reconectado): esquece o que foi lido dele.
    pub fn reset_device(&mut self) {
        *self = Self { page: self.page, default_delay: self.default_delay, ..Self::default() };
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some() || self.capture
    }
}

/// Completa a lista com macros vazias até [`MAX_MACROS`].
fn padded(macros: &[Macro]) -> Vec<Macro> {
    let mut out: Vec<Macro> = macros.iter().take(MAX_MACROS).cloned().collect();
    out.resize(MAX_MACROS, Vec::new());
    out
}

/// Nome de um uso HID da página teclado, com a legenda ABNT2.
pub fn usage_name(usage: u8) -> String {
    let Some(code) = hid_usage_to_evdev(usage as u16) else { return format!("Código 0x{usage:02x}") };
    let base = match legend(code) {
        l if l.is_empty() => "Espaço".to_string(),
        l => l,
    };
    match usage {
        0x53 => "Num Lock".into(),
        0x54..=0x63 | 0x85 => format!("Num {base}"),
        0xE3 | 0xE7 => if usage == 0xE7 { "Win dir.".into() } else { "Win".into() },
        0xE4..=0xE6 => format!("{base} dir."),
        _ => base,
    }
}

fn input_name(input: MacroInput) -> String {
    match input {
        MacroInput::Key(u) => usage_name(u),
        MacroInput::Mouse(b) => format!("Mouse: {}", onboard::mouse_name(b)),
    }
}

fn run_label(run: MacroRun) -> String {
    match run {
        MacroRun::Times(1) => "1 vez".into(),
        MacroRun::Times(n) => format!("{n} vezes"),
        MacroRun::UntilPressedAgain => "repete até apertar de novo".into(),
        MacroRun::WhileHeld => "repete enquanto apertada".into(),
    }
}

pub fn action_label(a: KeyAction, names: &[String]) -> String {
    match a {
        KeyAction::Disabled => "Nada (tecla desativada)".into(),
        KeyAction::Key { mods, usage } => format!("{}{}", onboard::mods_prefix(mods), usage_name(usage)),
        KeyAction::Media(u) => onboard::media_name(u),
        KeyAction::Mouse(b) => format!("Mouse: {}", onboard::mouse_name(b)),
        KeyAction::Power(n) => format!("Energia: {}", onboard::power_name(n)),
        KeyAction::Macro { slot, run } => {
            let name = names.get(slot as usize).cloned().unwrap_or_else(|| format!("Macro {}", slot + 1));
            format!("{name} ({})", run_label(run))
        }
        KeyAction::Fn => "Fn".into(),
        KeyAction::Other(c) => format!("Função desconhecida ({:02x} {:02x} {:02x})", c[0], c[1], c[2]),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Key,
    Media,
    Mouse,
    Power,
    Macro,
    Disabled,
}

const KINDS: &[(Kind, &str)] = &[
    (Kind::Key, "Tecla ou atalho"),
    (Kind::Media, "Multimídia"),
    (Kind::Mouse, "Botão do mouse"),
    (Kind::Power, "Energia"),
    (Kind::Macro, "Macro"),
    (Kind::Disabled, "Nada (desativar)"),
];

fn kind_of(a: KeyAction) -> Option<Kind> {
    Some(match a {
        KeyAction::Key { .. } => Kind::Key,
        KeyAction::Media(_) => Kind::Media,
        KeyAction::Mouse(_) => Kind::Mouse,
        KeyAction::Power(_) => Kind::Power,
        KeyAction::Macro { .. } => Kind::Macro,
        KeyAction::Disabled => Kind::Disabled,
        KeyAction::Fn | KeyAction::Other(_) => return None,
    })
}

fn default_for(kind: Kind) -> KeyAction {
    match kind {
        Kind::Key => KeyAction::Key { mods: 0, usage: 0x04 },
        Kind::Media => KeyAction::Media(0x00CD),
        Kind::Mouse => KeyAction::Mouse(1),
        Kind::Power => KeyAction::Power(2),
        Kind::Macro => KeyAction::Macro { slot: 0, run: MacroRun::Times(1) },
        Kind::Disabled => KeyAction::Disabled,
    }
}

/// Entradas que a macro deixa apertadas no fim (ficariam presas).
fn stuck_inputs(m: &Macro) -> Vec<MacroInput> {
    let mut held: Vec<MacroInput> = Vec::new();
    for s in m {
        if s.press {
            if !held.contains(&s.input) {
                held.push(s.input);
            }
        } else {
            held.retain(|&i| i != s.input);
        }
    }
    held
}

fn message(ui: &mut egui::Ui, state: &OnboardState) {
    if let Some((ok, msg)) = &state.message {
        ui.label(RichText::new(msg).color(if *ok { theme::OK } else { theme::ERROR }));
    }
}

impl App {
    fn ob_send(&self, c: OnboardCmd) {
        self.send(Cmd::Onboard(c));
    }

    fn device_id(&self) -> Option<String> {
        self.device.as_ref().map(|d| d.entry.id.clone())
    }

    /// Recebe o estado lido pelo motor; guarda a tabela original na primeira leitura.
    pub(super) fn onboard_sync(&mut self, state: OnboardState) {
        if let (Some(keys), Some(id)) = (&state.keys, self.device_id()) {
            let cfg = self.config.devices.entry(id).or_default();
            if onboard::cells_from_hex(&cfg.original_keys).is_none() {
                cfg.original_keys = onboard::cells_to_hex(keys);
                self.config.save();
            }
        }
        let loaded = state.macros.as_deref().map(padded);
        if loaded != self.ob.loaded_macros {
            // Só substitui o que está sendo editado se não havia mudanças.
            let dirty = self.ob.loaded_macros.as_ref().is_some_and(|l| *l != self.ob.macros);
            if let Some(l) = &loaded
                && !dirty
            {
                self.ob.macros = l.clone();
            }
            self.ob.loaded_macros = loaded;
        }
        self.ob.state = state;
    }

    /// Pede ao teclado o que a tela aberta precisa (uma vez por conexão).
    pub(super) fn onboard_load(&mut self) {
        match self.ob.page {
            Page::Keys if !self.ob.keys_requested => {
                self.ob.keys_requested = true;
                self.ob_send(OnboardCmd::LoadKeys);
            }
            Page::Macros if !self.ob.macros_requested => {
                self.ob.macros_requested = true;
                self.ob_send(OnboardCmd::LoadMacros);
            }
            _ => {}
        }
    }

    fn original_keys(&self) -> Option<Vec<Cell>> {
        let id = self.device_id()?;
        self.config.devices.get(&id).and_then(|d| onboard::cells_from_hex(&d.original_keys))
    }

    fn macro_names(&self) -> Vec<String> {
        let saved = self.device_id().and_then(|id| self.config.devices.get(&id)).map(|d| d.macro_names.clone()).unwrap_or_default();
        (0..MAX_MACROS)
            .map(|i| saved.get(i).filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("Macro {}", i + 1)))
            .collect()
    }

    /// Posição da tabela da tecla desenhada no LED `led`. Procura pela tecla
    /// que a posição digitava originalmente, porque o LED de algumas teclas
    /// fica em outra posição da matriz.
    pub(super) fn slot_for_led(&self, led: usize) -> Option<usize> {
        let table = self.original_keys().or_else(|| self.ob.state.keys.clone())?;
        let by_content = self
            .key_to_led
            .iter()
            .find(|&(_, &l)| l == led)
            .and_then(|(&code, _)| evdev_to_hid_usage(code))
            .and_then(|u| onboard::slot_of_usage(&table, u));
        by_content.or((led < onboard::SLOTS).then_some(led))
    }

    fn led_for_slot(&self, slot: usize, original: &[Cell]) -> usize {
        match KeyAction::decode(original[slot]) {
            KeyAction::Key { mods: 0, usage } => {
                hid_usage_to_evdev(usage as u16).and_then(|c| self.key_to_led.get(&c).copied()).unwrap_or(slot)
            }
            _ => slot,
        }
    }

    /// LEDs das teclas que fazem algo diferente do original.
    pub(super) fn changed_leds(&self) -> BTreeSet<usize> {
        let (Some(keys), Some(original)) = (&self.ob.state.keys, self.original_keys()) else { return BTreeSet::new() };
        (0..onboard::SLOTS).filter(|&s| keys[s] != original[s]).map(|s| self.led_for_slot(s, &original)).collect()
    }

    pub(super) fn select_key(&mut self, led: usize) {
        let Some(slot) = self.slot_for_led(led) else { return };
        self.ob.selected = Some((slot, led));
        self.ob.capture = false;
        if let Some(keys) = &self.ob.state.keys {
            self.ob.draft = KeyAction::decode(keys[slot]);
        }
    }

    /// Teclas apertadas enquanto se captura uma tecla ou se grava uma macro.
    pub(super) fn onboard_key_events(&mut self, events: &[(u16, bool)]) {
        for &(code, down) in events {
            let Some(usage) = evdev_to_hid_usage(code) else { continue };
            if self.ob.capture {
                if down {
                    let mods = match self.ob.draft {
                        KeyAction::Key { mods, .. } => mods,
                        _ => 0,
                    };
                    self.ob.draft = KeyAction::Key { mods, usage };
                    self.ob.capture = false;
                }
                continue;
            }
            let Some(last) = self.ob.recording else { continue };
            let steps = &mut self.ob.macros[self.ob.macro_sel];
            if steps.len() >= MAX_MACRO_STEPS {
                self.ob.recording = None;
                continue;
            }
            // A espera fica no passo anterior: é o tempo até este.
            if let Some(prev) = steps.last_mut() {
                let ms = (last.elapsed().as_millis() / 10 * 10).min(u16::MAX as u128) as u16;
                prev.delay_ms = ms;
            }
            steps.push(MacroStep { input: MacroInput::Key(usage), press: down, delay_ms: self.ob.default_delay });
            self.ob.recording = Some(Instant::now());
        }
    }

    // -----------------------------------------------------------------------
    // Teclas

    pub(super) fn keys_panel(&mut self, ui: &mut egui::Ui) {
        let Some(keys) = self.ob.state.keys.clone() else {
            ui.label(RichText::new("Teclas").font(semibold(20.0)));
            match &self.ob.state.message {
                Some((false, _)) => message(ui, &self.ob.state),
                _ => {
                    ui.label(RichText::new("Lendo o mapa de teclas do teclado...").weak());
                }
            }
            if ui.button("Ler de novo").clicked() {
                self.ob_send(OnboardCmd::LoadKeys);
            }
            return;
        };
        let original = self.original_keys().unwrap_or_else(|| keys.clone());
        let names = self.macro_names();
        let changed: Vec<usize> = (0..onboard::SLOTS).filter(|&s| keys[s] != original[s]).collect();

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(380.0);
                let Some((slot, _)) = self.ob.selected else {
                    ui.label(RichText::new("Teclas").font(semibold(20.0)));
                    ui.label(RichText::new("Clique numa tecla do desenho para mudar o que ela faz. A mudança fica gravada no teclado e vale em qualquer computador.").weak());
                    ui.add_space(8.0);
                    self.keys_summary(ui, &changed, &original);
                    return;
                };
                let current = KeyAction::decode(keys[slot]);
                let orig = KeyAction::decode(original[slot]);
                ui.label(RichText::new(action_label(orig, &names)).font(semibold(20.0)));
                let (row, col) = (slot / onboard::COLS, slot % onboard::COLS);
                ui.label(RichText::new(format!("Posição {} (linha {}, coluna {})", slot + 1, row + 1, col + 1)).weak());
                ui.add_space(6.0);
                ui.label(format!("Faz agora: {}", action_label(current, &names)));
                if current != orig && ui.button("Voltar ao original").clicked() {
                    self.ob_send(OnboardCmd::SetKeys(vec![(slot, original[slot])]));
                }
                ui.add_space(8.0);
                self.keys_summary(ui, &changed, &original);
            });
            ui.add_space(20.0);
            ui.separator();
            ui.add_space(20.0);
            ui.vertical(|ui| self.key_editor(ui, &keys, &names));
        });
    }

    fn keys_summary(&mut self, ui: &mut egui::Ui, changed: &[usize], original: &[Cell]) {
        ui.horizontal(|ui| {
            let text = match changed.len() {
                0 => "Nenhuma tecla alterada".to_string(),
                1 => "1 tecla alterada (em verde)".to_string(),
                n => format!("{n} teclas alteradas (em verde)"),
            };
            ui.label(RichText::new(text).weak());
            if !changed.is_empty() && ui.button("Restaurar todas").clicked() {
                self.ob_send(OnboardCmd::SetKeys(changed.iter().map(|&s| (s, original[s])).collect()));
            }
            if ui.button("Ler do teclado").clicked() {
                self.ob_send(OnboardCmd::LoadKeys);
            }
        });
        message(ui, &self.ob.state);
    }

    fn key_editor(&mut self, ui: &mut egui::Ui, keys: &[Cell], names: &[String]) {
        let Some((slot, _)) = self.ob.selected else { return };
        if KeyAction::decode(keys[slot]) == KeyAction::Fn {
            ui.label(RichText::new("Tecla Fn").font(semibold(20.0)));
            ui.label(RichText::new("A tecla Fn não pode ser mudada: é por ela que se chega às outras funções do teclado.").weak());
            return;
        }
        ui.label(RichText::new("Nova função").font(semibold(16.0)));
        ui.add_space(4.0);
        let mut draft = self.ob.draft;
        let mut capture = self.ob.capture;
        let kind = kind_of(draft);
        egui::Grid::new("key_editor").num_columns(2).spacing([20.0, 12.0]).show(ui, |ui| {
            ui.label(RichText::new("Tipo").weak());
            let label = kind.and_then(|k| KINDS.iter().find(|(x, _)| *x == k)).map_or("Escolha", |(_, n)| n);
            egui::ComboBox::from_id_salt("key_kind").selected_text(label).width(220.0).show_ui(ui, |ui| {
                for &(k, name) in KINDS {
                    if ui.selectable_label(kind == Some(k), name).clicked() && kind != Some(k) {
                        draft = default_for(k);
                    }
                }
            });
            ui.end_row();

            match &mut draft {
                KeyAction::Key { mods, usage } => {
                    ui.label(RichText::new("Tecla").weak());
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("key_usage").selected_text(usage_name(*usage)).width(160.0).height(320.0).show_ui(
                            ui,
                            |ui| {
                                for u in keyboard_usages() {
                                    ui.selectable_value(usage, u, usage_name(u));
                                }
                            },
                        );
                        let text = if capture { "Aperte a tecla..." } else { "Capturar" };
                        if ui.button(text).on_hover_text("A próxima tecla apertada no teclado vira a escolhida").clicked() {
                            capture = !capture;
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Junto com").weak());
                    ui.horizontal(|ui| {
                        for &(bit, name) in &MODIFIERS[..4] {
                            let mut on = *mods & bit != 0;
                            if ui.checkbox(&mut on, name).changed() {
                                *mods ^= bit;
                            }
                        }
                    });
                    ui.end_row();
                }
                KeyAction::Media(u) => {
                    ui.label(RichText::new("Tecla").weak());
                    egui::ComboBox::from_id_salt("media").selected_text(onboard::media_name(*u)).width(220.0).show_ui(ui, |ui| {
                        for &(v, name) in MEDIA_KEYS {
                            ui.selectable_value(u, v, name);
                        }
                    });
                    ui.end_row();
                }
                KeyAction::Mouse(b) => {
                    ui.label(RichText::new("Botão").weak());
                    egui::ComboBox::from_id_salt("mouse").selected_text(onboard::mouse_name(*b)).width(220.0).show_ui(ui, |ui| {
                        for &(v, name) in MOUSE_BUTTONS {
                            ui.selectable_value(b, v, name);
                        }
                    });
                    ui.end_row();
                }
                KeyAction::Power(n) => {
                    ui.label(RichText::new("Ação").weak());
                    egui::ComboBox::from_id_salt("power").selected_text(onboard::power_name(*n)).width(220.0).show_ui(ui, |ui| {
                        for &(v, name) in POWER_ACTIONS {
                            ui.selectable_value(n, v, name);
                        }
                    });
                    ui.end_row();
                }
                KeyAction::Macro { slot: m, run } => {
                    ui.label(RichText::new("Macro").weak());
                    egui::ComboBox::from_id_salt("macro_slot").selected_text(names[*m as usize % MAX_MACROS].clone()).width(220.0).show_ui(
                        ui,
                        |ui| {
                            for (i, name) in names.iter().enumerate() {
                                ui.selectable_value(m, i as u8, name);
                            }
                        },
                    );
                    ui.end_row();
                    ui.label(RichText::new("Executar").weak());
                    ui.horizontal(|ui| {
                        let mut times = matches!(run, MacroRun::Times(_));
                        let options = [(true, "N vezes"), (false, "Repetindo")];
                        for (value, name) in options {
                            if ui.radio(times == value, name).clicked() {
                                times = value;
                                *run = if value { MacroRun::Times(1) } else { MacroRun::WhileHeld };
                            }
                        }
                        match run {
                            MacroRun::Times(n) => {
                                ui.add(egui::DragValue::new(n).range(1..=255).suffix("×"));
                            }
                            _ => {
                                egui::ComboBox::from_id_salt("macro_repeat").selected_text(run_label(*run)).show_ui(ui, |ui| {
                                    ui.selectable_value(run, MacroRun::WhileHeld, run_label(MacroRun::WhileHeld));
                                    ui.selectable_value(run, MacroRun::UntilPressedAgain, run_label(MacroRun::UntilPressedAgain));
                                });
                            }
                        }
                    });
                    ui.end_row();
                }
                KeyAction::Disabled => {
                    ui.label("");
                    ui.label(RichText::new("A tecla não vai fazer nada.").weak());
                    ui.end_row();
                }
                KeyAction::Fn | KeyAction::Other(_) => {
                    ui.label("");
                    ui.label(RichText::new("Escolha o tipo para montar a nova função.").weak());
                    ui.end_row();
                }
            }
        });

        if let KeyAction::Macro { slot: m, .. } = draft
            && let Some(loaded) = &self.ob.loaded_macros
            && loaded.get(m as usize).is_none_or(|x| x.is_empty())
        {
            ui.label(RichText::new("Esta macro está vazia no teclado; grave-a na aba Macros.").color(theme::WARN));
        }
        if capture && cfg!(target_os = "macos") {
            ui.label(RichText::new("No macOS o keyglow ainda não lê o teclado; escolha a tecla na lista.").color(theme::WARN));
            capture = false;
        }

        ui.add_space(10.0);
        let cell = draft.encode();
        let can_save = kind_of(draft).is_some() && cell != keys[slot];
        if ui.add_enabled(can_save, egui::Button::new("Gravar no teclado")).clicked() {
            self.ob_send(OnboardCmd::SetKeys(vec![(slot, cell)]));
        }
        self.ob.draft = draft;
        self.ob.capture = capture;
    }

    // -----------------------------------------------------------------------
    // Macros

    pub(super) fn macros_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Macros").font(semibold(22.0)));
        ui.label(
            RichText::new(
                "Sequências de teclas gravadas no teclado. Depois de gravar, ligue uma macro a uma tecla na aba Teclas. O teclado guarda só os passos; os nomes ficam no keyglow.",
            )
            .weak(),
        );
        ui.add_space(10.0);
        let Some(loaded) = self.ob.loaded_macros.clone() else {
            match &self.ob.state.message {
                Some((false, _)) => message(ui, &self.ob.state),
                _ => {
                    ui.label(RichText::new("Lendo as macros do teclado...").weak());
                }
            }
            if ui.button("Ler de novo").clicked() {
                self.ob_send(OnboardCmd::LoadMacros);
            }
            return;
        };
        let names = self.macro_names();

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(260.0);
                for (i, name) in names.iter().enumerate() {
                    let n = self.ob.macros[i].len();
                    let text = if n == 0 { format!("{}. {name}  ·  vazia", i + 1) } else { format!("{}. {name}  ·  {n} passos", i + 1) };
                    if ui.selectable_label(self.ob.macro_sel == i, text).clicked() {
                        self.ob.macro_sel = i;
                        self.ob.recording = None;
                    }
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Nome").weak());
                let mut name = names[self.ob.macro_sel].clone();
                if ui.text_edit_singleline(&mut name).changed()
                    && let Some(id) = self.device_id()
                {
                    let cfg = self.config.devices.entry(id).or_default();
                    cfg.macro_names.resize(MAX_MACROS, String::new());
                    cfg.macro_names[self.ob.macro_sel] = name;
                    self.config.save();
                }
            });
            ui.add_space(20.0);
            ui.separator();
            ui.add_space(20.0);
            ui.vertical(|ui| self.macro_editor(ui, &loaded));
        });
    }

    fn macro_editor(&mut self, ui: &mut egui::Ui, loaded: &[Macro]) {
        let sel = self.ob.macro_sel;
        // Ações dos botões de cada linha, aplicadas depois de desenhar a lista.
        let mut remove = None;
        let mut swap = None;
        {
            let steps = &mut self.ob.macros[sel];
            egui::ScrollArea::vertical().max_height((ui.available_height() - 170.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
                if steps.is_empty() {
                    ui.label(RichText::new("Macro vazia. Adicione passos abaixo ou grave do teclado.").weak());
                }
                egui::Grid::new("macro_steps").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                    let len = steps.len();
                    for (i, step) in steps.iter_mut().enumerate() {
                        ui.label(RichText::new(format!("{}", i + 1)).weak());
                        egui::ComboBox::from_id_salt(("press", i)).selected_text(if step.press { "Apertar" } else { "Soltar" }).width(90.0).show_ui(
                            ui,
                            |ui| {
                                ui.selectable_value(&mut step.press, true, "Apertar");
                                ui.selectable_value(&mut step.press, false, "Soltar");
                            },
                        );
                        ui.label(input_name(step.input));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("depois espera").weak());
                            ui.add(egui::DragValue::new(&mut step.delay_ms).range(0..=u16::MAX).suffix(" ms"));
                        });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                                swap = Some((i - 1, i));
                            }
                            if ui.add_enabled(i + 1 < len, egui::Button::new("↓").small()).clicked() {
                                swap = Some((i, i + 1));
                            }
                            if ui.small_button("✕").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some((a, b)) = swap {
                steps.swap(a, b);
            }
            if let Some(i) = remove {
                steps.remove(i);
            }
        }

        ui.add_space(8.0);
        let full = self.ob.macros[sel].len() + 2 > MAX_MACRO_STEPS;
        let delay = self.ob.default_delay;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("new_key").selected_text(usage_name(self.ob.new_key)).width(130.0).height(320.0).show_ui(ui, |ui| {
                for u in keyboard_usages() {
                    ui.selectable_value(&mut self.ob.new_key, u, usage_name(u));
                }
            });
            let input = MacroInput::Key(self.ob.new_key);
            let steps = &mut self.ob.macros[sel];
            if ui.add_enabled(!full, egui::Button::new("+ Tocar")).on_hover_text("Aperta e solta a tecla").clicked() {
                steps.push(MacroStep { input, press: true, delay_ms: delay });
                steps.push(MacroStep { input, press: false, delay_ms: delay });
            }
            if ui.add_enabled(!full, egui::Button::new("+ Apertar")).clicked() {
                steps.push(MacroStep { input, press: true, delay_ms: delay });
            }
            if ui.add_enabled(!full, egui::Button::new("+ Soltar")).clicked() {
                steps.push(MacroStep { input, press: false, delay_ms: delay });
            }
            ui.add_space(14.0);
            egui::ComboBox::from_id_salt("new_mouse").selected_text(onboard::mouse_name(self.ob.new_mouse)).width(130.0).show_ui(ui, |ui| {
                for &(v, name) in MOUSE_BUTTONS {
                    ui.selectable_value(&mut self.ob.new_mouse, v, name);
                }
            });
            let mouse = MacroInput::Mouse(self.ob.new_mouse);
            let steps = &mut self.ob.macros[sel];
            if ui.add_enabled(!full, egui::Button::new("+ Clique")).clicked() {
                steps.push(MacroStep { input: mouse, press: true, delay_ms: delay });
                steps.push(MacroStep { input: mouse, press: false, delay_ms: delay });
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Espera padrão").weak());
            ui.add(egui::DragValue::new(&mut self.ob.default_delay).range(0..=u16::MAX).suffix(" ms"));
            ui.add_space(14.0);
            if cfg!(target_os = "macos") {
                ui.label(RichText::new("Gravar do teclado: ainda não disponível no macOS").weak());
            } else if self.ob.recording.is_some() {
                if ui.button("■ Parar gravação").clicked() {
                    self.ob.recording = None;
                }
                ui.label(RichText::new("Digite a sequência no teclado...").color(theme::WARN));
            } else if ui.add_enabled(!full, egui::Button::new("● Gravar do teclado")).clicked() {
                self.input.lock().unwrap().gui_events.clear();
                self.ob.recording = Some(Instant::now());
            }
            if ui.add_enabled(!self.ob.macros[sel].is_empty(), egui::Button::new("Limpar")).clicked() {
                self.ob.macros[sel].clear();
                self.ob.recording = None;
            }
        });

        // Validação e gravação de todas as macros de uma vez.
        ui.add_space(8.0);
        let mut problems = Vec::new();
        for (i, m) in self.ob.macros.iter().enumerate() {
            if m.len() > MAX_MACRO_STEPS {
                problems.push(format!("macro {} tem mais de {MAX_MACRO_STEPS} passos", i + 1));
            }
            let stuck = stuck_inputs(m);
            if !stuck.is_empty() {
                let keys: Vec<String> = stuck.into_iter().map(input_name).collect();
                problems.push(format!("macro {} termina com {} apertado (falta soltar)", i + 1, keys.join(", ")));
            }
        }
        let dirty = self.ob.macros != loaded;
        for p in &problems {
            ui.label(RichText::new(p).color(theme::WARN));
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(dirty && problems.is_empty(), egui::Button::new("Gravar macros no teclado")).clicked() {
                self.ob.recording = None;
                self.ob_send(OnboardCmd::SaveMacros(self.ob.macros.clone()));
            }
            if ui.add_enabled(dirty, egui::Button::new("Desfazer mudanças")).clicked() {
                self.ob.macros = loaded.to_vec();
                self.ob.recording = None;
            }
            if dirty {
                ui.label(RichText::new("Há mudanças não gravadas").color(theme::WARN));
            }
            message(ui, &self.ob.state);
        });
    }

    // -----------------------------------------------------------------------
    // Teclado

    pub(super) fn keyboard_page(&mut self, ui: &mut egui::Ui) {
        let Some(device) = self.device.clone() else { return };
        ui.label(RichText::new(&device.entry.name).font(semibold(22.0)));
        ui.add_space(6.0);
        egui::Grid::new("device_info").num_columns(2).spacing([28.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Tipo").weak());
            ui.label(device.entry.kind.label());
            ui.end_row();
            ui.label(RichText::new("Identificação").weak());
            ui.label(&device.entry.id);
            ui.end_row();
            ui.label(RichText::new("Versão do firmware").weak());
            ui.label(match device.firmware {
                Some(v) => format!("{}.{:02x}", v >> 8, v & 0xFF),
                None => "não informada".into(),
            });
            ui.end_row();
        });
        ui.add_space(16.0);
        ui.label(RichText::new("Gravado no teclado").font(semibold(16.0)));
        ui.label(RichText::new("Efeito de luz, o que cada tecla faz e as macros ficam na memória do teclado e continuam valendo com o keyglow fechado ou em outro computador.").weak());
        ui.add_space(16.0);
        ui.label(RichText::new("Padrão de fábrica").font(semibold(16.0)));
        ui.label(RichText::new("Apaga as macros, desfaz todas as mudanças de teclas e volta a luz ao efeito de fábrica.").weak());
        ui.add_space(6.0);
        if ui.button("Voltar ao padrão de fábrica...").clicked() {
            self.ob.confirm_reset = true;
        }
        message(ui, &self.ob.state);

        if self.ob.confirm_reset {
            let modal = egui::Modal::new(egui::Id::unique("confirm_reset")).show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Voltar ao padrão de fábrica?").font(semibold(18.0)));
                ui.label("As macros e as mudanças de teclas serão apagadas do teclado. Isso não pode ser desfeito.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancelar").clicked() {
                        self.ob.confirm_reset = false;
                    }
                    if ui.button(RichText::new("Voltar ao padrão").color(theme::ERROR)).clicked() {
                        self.ob.confirm_reset = false;
                        // Depois do reset, a tabela de fábrica passa a ser a original.
                        if let Some(id) = self.device_id() {
                            let cfg = self.config.devices.entry(id).or_default();
                            cfg.original_keys.clear();
                            cfg.macro_names.clear();
                            self.config.save();
                        }
                        self.ob_send(OnboardCmd::FactoryReset);
                    }
                });
            });
            if modal.should_close() {
                self.ob.confirm_reset = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_keys_left_pressed() {
        let a = MacroInput::Key(0x04);
        let shift = MacroInput::Key(0xE1);
        let step = |input, press| MacroStep { input, press, delay_ms: 0 };
        assert!(stuck_inputs(&vec![step(a, true), step(a, false)]).is_empty());
        assert_eq!(stuck_inputs(&vec![step(shift, true), step(a, true), step(a, false)]), vec![shift]);
        assert!(stuck_inputs(&vec![step(a, false)]).is_empty());
    }

    #[test]
    fn names() {
        assert_eq!(usage_name(0x04), "A");
        assert_eq!(usage_name(0x2C), "Espaço");
        assert_eq!(usage_name(0x59), "Num 1");
        assert_eq!(usage_name(0xE5), "Shift dir.");
        assert_eq!(action_label(KeyAction::Key { mods: 0x03, usage: 0x06 }, &[]), "Ctrl+Shift+C");
        assert_eq!(
            action_label(KeyAction::Macro { slot: 1, run: MacroRun::Times(3) }, &["a".into(), "Copiar".into()]),
            "Copiar (3 vezes)"
        );
    }
}
