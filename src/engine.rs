//! Motor de efeitos: thread que é dona do teclado.
//!
//! - Procura teclados compatíveis e conecta no escolhido (`drivers`).
//! - Efeito de fábrica: devolve o controle ao teclado (e, nos EVision, grava
//!   o efeito escolhido).
//! - Efeito de software: põe o teclado no modo direto e envia ~30 imagens/s.

use crate::drivers::{self, DeviceEntry, Driver, Grid, HwMode, Rgb, Source};
use crate::effects::{AudioData, Ctx, Effect, KeyEvent, Params, EFFECTS};
use crate::input::InputState;
use crate::onboard::{self, Cell, Macro};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAME_TIME: Duration = Duration::from_millis(33);
const RECONNECT_TIME: Duration = Duration::from_secs(2);

pub enum Cmd {
    /// Volta ao efeito do teclado; nos EVision, grava e ativa `HwMode`.
    Hardware(HwMode),
    /// Ativa o efeito de software de índice `n` em [`EFFECTS`].
    Software(usize),
    /// Troca para o teclado com este id.
    Select(String),
    /// Procura teclados de novo.
    Rescan,
    /// Lê ou grava a memória do teclado (teclas, macros).
    Onboard(OnboardCmd),
    /// Restaura o efeito de fábrica e encerra a thread.
    Shutdown,
}

pub enum OnboardCmd {
    LoadKeys,
    /// Altera estas posições (linha por linha) da tabela de teclas.
    SetKeys(Vec<(usize, Cell)>),
    LoadMacros,
    SaveMacros(Vec<Macro>),
    FactoryReset,
}

/// O que a interface sabe da memória do teclado conectado.
#[derive(Clone, Debug, Default)]
pub struct OnboardState {
    pub keys: Option<Vec<Cell>>,
    pub macros: Option<Vec<Macro>>,
    /// Resultado da última operação: (deu certo, mensagem).
    pub message: Option<(bool, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting(String),
    Connected,
}

/// O teclado conectado, como a interface precisa vê-lo.
#[derive(Clone, Debug)]
pub struct DeviceView {
    pub entry: DeviceEntry,
    pub grid: Grid,
    /// Efeitos de fábrica; vazio = o teclado não tem.
    pub hw_modes: &'static [(u8, &'static str)],
    /// Mapa de teclas conhecido sem calibração.
    pub default_key_map: BTreeMap<u16, usize>,
    /// O keyglow sabe mexer nas teclas e macros gravadas neste teclado.
    pub onboard: bool,
    pub firmware: Option<u16>,
}

/// Estado compartilhado entre o motor e a interface.
pub struct Shared {
    pub params: Params,
    pub key_to_led: BTreeMap<u16, usize>,
    /// Durante a calibração: célula que deve piscar.
    pub calibrate_led: Option<usize>,
    /// Última imagem enviada (para a prévia na tela).
    pub preview: Vec<Rgb>,
    pub status: Status,
    /// Efeito de fábrica lido do teclado ao conectar.
    pub device_hw: Option<HwMode>,
    pub fps: f32,
    /// Teclados encontrados na última busca.
    pub devices: Vec<DeviceEntry>,
    /// Teclado conectado.
    pub device: Option<DeviceView>,
    /// Conta as conexões (a interface relê o efeito do teclado a cada uma).
    pub connections: u64,
    pub onboard: OnboardState,
}

impl Shared {
    pub fn new(params: Params) -> Self {
        Self {
            params,
            key_to_led: BTreeMap::new(),
            calibrate_led: None,
            preview: vec![[0; 3]; Grid::default().len()],
            status: Status::Connecting("procurando teclados...".into()),
            device_hw: None,
            fps: 0.0,
            devices: Vec::new(),
            device: None,
            connections: 0,
            onboard: OnboardState::default(),
        }
    }
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    audio: Arc<Mutex<AudioData>>,
    audio_wanted: Arc<AtomicBool>,
    rx: Receiver<Cmd>,

    kb: Option<Box<dyn Driver>>,
    grid: Grid,
    /// Id do teclado escolhido (`None` = o primeiro encontrado).
    wanted: Option<String>,
    last_attempt: Option<Instant>,
    /// O teclado está no modo direto recebendo imagens?
    in_direct: bool,
    hw: HwMode,
    effect_index: Option<usize>,
    effect: Option<Box<dyn Effect>>,
    effect_start: Instant,
    last_sent: Option<(Vec<Rgb>, Instant)>,
}

pub struct Start {
    pub hw: HwMode,
    pub software_effect: Option<usize>,
    pub device: Option<String>,
}

pub fn spawn(
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    audio: Arc<Mutex<AudioData>>,
    audio_wanted: Arc<AtomicBool>,
    start: Start,
) -> (Sender<Cmd>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let mut engine = Engine {
        shared,
        input,
        audio,
        audio_wanted,
        rx,
        kb: None,
        grid: Grid::default(),
        wanted: start.device,
        last_attempt: None,
        in_direct: false,
        hw: start.hw,
        effect_index: start.software_effect,
        effect: start.software_effect.and_then(|i| EFFECTS.get(i)).map(|e| (e.build)()),
        effect_start: Instant::now(),
        last_sent: None,
    };
    let handle = thread::Builder::new()
        .name("engine".into())
        .spawn(move || engine.run())
        .expect("não consegui criar a thread do motor");
    (tx, handle)
}

impl Engine {
    fn run(&mut self) {
        let mut last = Instant::now();
        // Contagem real de quadros por segundo (para a interface).
        let (mut frames, mut window) = (0u32, Instant::now());
        loop {
            let start = Instant::now();
            let dt = start.duration_since(last).as_secs_f32();
            last = start;

            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    Cmd::Hardware(m) => {
                        self.hw = m;
                        self.effect_index = None;
                        self.effect = None;
                        self.leave_direct();
                        if let Some(kb) = self.kb.as_mut()
                            && let Err(e) = kb.set_hw_mode(&m)
                        {
                            self.disconnect(e.to_string());
                        }
                    }
                    Cmd::Software(i) => {
                        self.effect_index = Some(i);
                        self.effect = EFFECTS.get(i).map(|e| (e.build)());
                        self.effect_start = Instant::now();
                    }
                    Cmd::Select(id) => {
                        self.leave_direct();
                        self.kb = None;
                        self.wanted = Some(id);
                        self.last_attempt = None;
                    }
                    Cmd::Onboard(c) => self.onboard_cmd(c),
                    Cmd::Rescan => {
                        if self.kb.is_some() {
                            self.shared.lock().unwrap().devices = drivers::scan().0;
                        } else {
                            self.last_attempt = None;
                        }
                    }
                    Cmd::Shutdown => {
                        self.leave_direct();
                        self.audio_wanted.store(false, Ordering::Relaxed);
                        return;
                    }
                }
            }

            self.ensure_connected();
            self.tick(dt);

            if let Some(rest) = FRAME_TIME.checked_sub(start.elapsed()) {
                thread::sleep(rest);
            }
            frames += 1;
            if window.elapsed() >= Duration::from_secs(1) {
                self.shared.lock().unwrap().fps = frames as f32 / window.elapsed().as_secs_f32();
                (frames, window) = (0, Instant::now());
            }
        }
    }

    fn ensure_connected(&mut self) {
        if self.kb.is_some() || self.last_attempt.is_some_and(|t| t.elapsed() < RECONNECT_TIME) {
            return;
        }
        self.last_attempt = Some(Instant::now());
        let (devices, errors) = drivers::scan();
        let chosen = self
            .wanted
            .as_ref()
            .and_then(|id| devices.iter().find(|d| &d.id == id))
            // Sem escolha salva, só conecta sozinho num modelo testado; os
            // outros esperam o usuário escolher na lista.
            .or_else(|| devices.iter().find(|d| d.tested))
            .cloned();
        let devices_found = devices.len();
        self.shared.lock().unwrap().devices = devices;

        let Some(entry) = chosen else {
            let msg = if !errors.is_empty() {
                errors.join("; ")
            } else if devices_found > 0 {
                "escolha o teclado na lista".into()
            } else {
                "nenhum teclado compatível encontrado".into()
            };
            self.shared.lock().unwrap().status = Status::Connecting(msg);
            return;
        };
        let saved = self.hw;
        match drivers::open(&entry).and_then(|mut kb| kb.connect(&saved).map(|hw| (kb, hw))) {
            Ok((mut kb, device_hw)) => {
                if let Some(hw) = device_hw {
                    self.hw = hw;
                }
                self.grid = kb.grid().clone();
                // Efeitos guardam estado do tamanho da grade: recomeça.
                self.effect = self.effect_index.and_then(|i| EFFECTS.get(i)).map(|e| (e.build)());
                self.input.lock().unwrap().target = match &entry.source {
                    Source::Hid(info) => Some((info.vid, info.pid)),
                    Source::OpenRgb { .. } => None,
                };
                let view = DeviceView {
                    entry,
                    grid: self.grid.clone(),
                    hw_modes: kb.hw_modes(),
                    default_key_map: kb.default_key_map(),
                    onboard: kb.onboard().is_some(),
                    firmware: kb.firmware_version(),
                };
                let mut sh = self.shared.lock().unwrap();
                sh.device_hw = device_hw;
                sh.device = Some(view);
                sh.connections += 1;
                sh.onboard = OnboardState::default();
                sh.preview = vec![[0; 3]; self.grid.len()];
                sh.status = Status::Connected;
                drop(sh);
                self.kb = Some(kb);
                self.in_direct = false;
            }
            Err(e) => self.shared.lock().unwrap().status = Status::Connecting(format!("{}: {e}", entry.name)),
        }
    }

    fn disconnect(&mut self, reason: String) {
        self.kb = None;
        self.in_direct = false;
        self.last_attempt = Some(Instant::now());
        self.shared.lock().unwrap().status = Status::Connecting(format!("conexão perdida: {reason}"));
    }

    /// Executa um pedido sobre a memória do teclado e publica o resultado.
    fn onboard_cmd(&mut self, c: OnboardCmd) {
        let writes = !matches!(c, OnboardCmd::LoadKeys | OnboardCmd::LoadMacros);
        if writes {
            // Nada de imagens do modo direto no meio de uma gravação.
            self.leave_direct();
        }
        let reset = matches!(c, OnboardCmd::FactoryReset);
        let result = match self.kb.as_mut().map(|kb| kb.onboard()) {
            Some(Some(ob)) => run_onboard(ob, c),
            Some(None) => Err(std::io::Error::other("o teclado conectado não guarda teclas e macros no formato que o keyglow conhece")),
            None => Err(std::io::Error::other("teclado desconectado")),
        };
        let mut sh = self.shared.lock().unwrap();
        match result {
            Ok(done) => {
                if let Some(keys) = done.keys {
                    sh.onboard.keys = Some(keys);
                }
                if let Some(macros) = done.macros {
                    sh.onboard.macros = Some(macros);
                }
                if let Some(msg) = done.message {
                    sh.onboard.message = Some((true, msg));
                }
            }
            Err(e) => sh.onboard.message = Some((false, e.to_string())),
        }
        drop(sh);
        if reset {
            // O teclado reinicia: conecta de novo para ler o estado de fábrica.
            thread::sleep(Duration::from_secs(2));
            self.kb = None;
            self.in_direct = false;
            self.last_attempt = None;
        }
    }

    fn leave_direct(&mut self) {
        if !self.in_direct {
            return;
        }
        self.in_direct = false;
        if let Some(kb) = self.kb.as_mut() {
            let _ = kb.leave_direct(&self.hw);
        }
    }

    fn tick(&mut self, dt: f32) {
        // Pega os eventos de tecla e o estado compartilhado de uma vez só,
        // para segurar os locks o mínimo possível.
        let (raw_events, pressed_codes) = {
            let mut input = self.input.lock().unwrap();
            (std::mem::take(&mut input.engine_events), input.pressed.clone())
        };
        let (params, mut key_to_led, calibrate, has_hw) = {
            let sh = self.shared.lock().unwrap();
            let has_hw = sh.device.as_ref().is_none_or(|d| !d.hw_modes.is_empty());
            (sh.params.clone(), sh.key_to_led.clone(), sh.calibrate_led, has_hw)
        };
        let len = self.grid.len();
        key_to_led.retain(|_, led| *led < len);
        let events: Vec<KeyEvent> = raw_events
            .iter()
            .map(|&(code, pressed)| KeyEvent { code, led: key_to_led.get(&code).copied(), pressed })
            .collect();
        let pressed: Vec<usize> = pressed_codes.iter().filter_map(|c| key_to_led.get(c).copied()).collect();

        let mut frame: Vec<Rgb> = vec![[0; 3]; len];
        let software = if let Some(led) = calibrate.filter(|&l| l < len) {
            // Calibração: LEDs já mapeados em verde fraco e o atual piscando.
            for &mapped in key_to_led.values() {
                frame[mapped] = [0, 40, 0];
            }
            let t = self.effect_start.elapsed().as_secs_f32();
            let v = (0.6 + 0.4 * (t * 8.0).sin()) * 255.0;
            frame[led] = [v as u8; 3];
            true
        } else if let Some(effect) = self.effect.as_mut() {
            self.audio_wanted.store(effect.uses_audio(), Ordering::Relaxed);
            let audio = *self.audio.lock().unwrap();
            let ctx = Ctx {
                time: self.effect_start.elapsed().as_secs_f32(),
                dt,
                params: &params,
                grid: &self.grid,
                events: &events,
                pressed: &pressed,
                audio: &audio,
            };
            effect.render(&ctx, &mut frame);
            for px in frame.iter_mut() {
                *px = crate::effects::scale(*px, params.brightness);
            }
            true
        } else {
            false
        };
        if !software {
            self.audio_wanted.store(false, Ordering::Relaxed);
        }

        if software {
            if let Err(e) = self.send(&frame) {
                self.disconnect(e.to_string());
            }
        } else {
            self.leave_direct();
            if has_hw {
                // Prévia aproximada do efeito de fábrica.
                let k = self.hw.brightness as f32 / 4.0;
                let color = if self.hw.random { [200, 200, 200] } else { self.hw.color };
                let dim = if self.hw.mode == drivers::evision::MODE_STATIC { 1.0 } else { 0.6 };
                frame.fill(crate::effects::scale(color, k * dim));
            } else {
                // O teclado mostra o efeito dele; não sabemos qual é.
                frame.fill([60, 60, 70]);
            }
        }
        self.shared.lock().unwrap().preview = frame;
    }

    fn send(&mut self, frame: &[Rgb]) -> std::io::Result<()> {
        let Some(kb) = self.kb.as_mut() else { return Ok(()) };
        if !self.in_direct {
            kb.enter_direct(&self.hw)?;
            self.in_direct = true;
            self.last_sent = None;
        }
        // Imagem igual à anterior: só reenvia se o firmware precisar.
        if let Some((last, at)) = &self.last_sent
            && last.as_slice() == frame
            && kb.keepalive().is_none_or(|k| at.elapsed() < k)
        {
            return Ok(());
        }
        kb.send_frame(frame)?;
        self.last_sent = Some((frame.to_vec(), Instant::now()));
        Ok(())
    }
}

#[derive(Default)]
struct OnboardDone {
    keys: Option<Vec<Cell>>,
    macros: Option<Vec<Macro>>,
    message: Option<String>,
}

fn run_onboard(ob: &mut dyn drivers::Onboard, c: OnboardCmd) -> std::io::Result<OnboardDone> {
    let mut done = OnboardDone::default();
    match c {
        OnboardCmd::LoadKeys => done.keys = Some(ob.read_keys()?),
        OnboardCmd::SetKeys(edits) => {
            // Sempre a partir do que está no teclado agora: só as posições
            // pedidas mudam, o resto (inclusive a tecla Fn) fica igual.
            let mut table = ob.read_keys()?;
            let changed = onboard::apply_key_edits(&mut table, &edits);
            if changed > 0 {
                ob.write_keys(&table)?;
            }
            let after = ob.read_keys()?;
            if after != table {
                done.keys = Some(after);
                return Err(std::io::Error::other("o teclado não guardou o mapa de teclas como enviado"));
            }
            done.keys = Some(after);
            done.message = Some(match changed {
                0 => "nada mudou".into(),
                1 => "1 tecla gravada no teclado".into(),
                n => format!("{n} teclas gravadas no teclado"),
            });
        }
        OnboardCmd::LoadMacros => {
            let block = ob.read_macros()?;
            done.macros = Some(onboard::decode_macros(&block).map_err(std::io::Error::other)?);
        }
        OnboardCmd::SaveMacros(macros) => {
            let block = onboard::encode_macros(&macros);
            ob.write_macros(&block)?;
            let back = ob.read_macros()?;
            if back.get(..block.len()) != Some(block.as_slice()) {
                return Err(std::io::Error::other("o teclado não guardou as macros como enviadas"));
            }
            done.macros = Some(onboard::decode_macros(&back).map_err(std::io::Error::other)?);
            done.message = Some("macros gravadas no teclado".into());
        }
        OnboardCmd::FactoryReset => {
            ob.factory_reset()?;
            done.message = Some("teclado voltou ao padrão de fábrica".into());
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::evision::Evision;
    use crate::hid::mock::MockEvision;
    use crate::onboard::{KeyAction, MacroInput, MacroStep};

    fn mock_kb() -> Evision {
        let mut kb = Evision::with_io(Box::new(MockEvision::new(Arc::new(Mutex::new(Vec::new())))), 0);
        kb.enable_onboard_for_tests();
        kb
    }

    #[test]
    fn set_keys_reads_modifies_and_verifies() {
        let mut kb = mock_kb();
        let ob = kb.onboard().unwrap();
        let play = KeyAction::Media(0x00CD).encode();
        let fnk = KeyAction::Fn.encode();
        let done = run_onboard(ob, OnboardCmd::SetKeys(vec![(5, play), (110, play), (6, fnk)])).unwrap();
        let keys = done.keys.unwrap();
        assert_eq!(keys[5], play);
        assert_eq!(keys[110], fnk, "a tecla Fn não muda");
        assert_ne!(keys[6], fnk);
        assert_eq!(done.message.as_deref(), Some("1 tecla gravada no teclado"));
        // Sem mudanças, nada é gravado.
        let again = run_onboard(ob, OnboardCmd::SetKeys(vec![(5, play)])).unwrap();
        assert_eq!(again.message.as_deref(), Some("nada mudou"));
    }

    #[test]
    fn macros_round_trip_through_the_keyboard() {
        let mut kb = mock_kb();
        let ob = kb.onboard().unwrap();
        assert_eq!(run_onboard(ob, OnboardCmd::LoadMacros).unwrap().macros, Some(Vec::new()));
        let m = vec![MacroStep { input: MacroInput::Key(0x05), press: true, delay_ms: 10 }];
        let saved = run_onboard(ob, OnboardCmd::SaveMacros(vec![Vec::new(), m.clone()])).unwrap();
        assert_eq!(saved.macros, Some(vec![Vec::new(), m]));
        run_onboard(ob, OnboardCmd::FactoryReset).unwrap();
    }
}
