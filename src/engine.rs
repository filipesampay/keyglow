//! Motor de efeitos: thread que é dona do teclado.
//!
//! - Efeito de fábrica: grava a configuração e deixa o teclado animar sozinho.
//! - Efeito de software: põe o teclado no modo Custom e envia ~30 imagens/s.

use crate::device::{Frame, HwMode, Keyboard, LEDS, MODE_CUSTOM, MODE_STATIC};
use crate::effects::{AudioData, Ctx, Effect, KeyEvent, Params, EFFECTS};
use crate::input::InputState;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAME_TIME: Duration = Duration::from_millis(33);
const RECONNECT_TIME: Duration = Duration::from_secs(2);
/// Reenvia a mesma imagem neste intervalo, senão o teclado sai do modo direto
/// (~0,5 s sem imagens). Imagens diferentes são enviadas na hora.
const KEEPALIVE_TIME: Duration = Duration::from_millis(150);

pub enum Cmd {
    /// Grava e ativa um efeito de fábrica.
    Hardware(HwMode),
    /// Ativa o efeito de software de índice `n` em [`EFFECTS`].
    Software(usize),
    /// Restaura o efeito de fábrica e encerra a thread.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting(String),
    Connected,
}

/// Estado compartilhado entre o motor e a interface.
pub struct Shared {
    pub params: Params,
    pub key_to_led: BTreeMap<u16, usize>,
    /// Durante a calibração: LED que deve piscar.
    pub calibrate_led: Option<usize>,
    /// Última imagem enviada (para a prévia na tela).
    pub preview: Frame,
    pub status: Status,
    /// Efeito de fábrica lido do teclado ao conectar.
    pub device_hw: Option<HwMode>,
    pub fps: f32,
}

impl Shared {
    pub fn new(params: Params, key_to_led: BTreeMap<u16, usize>) -> Self {
        Self {
            params,
            key_to_led,
            calibrate_led: None,
            preview: [[0; 3]; LEDS],
            status: Status::Connecting("procurando o teclado...".into()),
            device_hw: None,
            fps: 0.0,
        }
    }
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    audio: Arc<Mutex<AudioData>>,
    audio_wanted: Arc<AtomicBool>,
    rx: Receiver<Cmd>,

    kb: Option<Keyboard>,
    last_attempt: Option<Instant>,
    /// O teclado está no modo Custom recebendo imagens?
    in_direct: bool,
    hw: HwMode,
    effect: Option<Box<dyn Effect>>,
    effect_start: Instant,
    last_sent: Option<(Frame, Instant)>,
}

pub fn spawn(
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputState>>,
    audio: Arc<Mutex<AudioData>>,
    audio_wanted: Arc<AtomicBool>,
    hw: HwMode,
    software_effect: Option<usize>,
) -> (Sender<Cmd>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let mut engine = Engine {
        shared,
        input,
        audio,
        audio_wanted,
        rx,
        kb: None,
        last_attempt: None,
        in_direct: false,
        hw,
        effect: software_effect.and_then(|i| EFFECTS.get(i)).map(|e| (e.build)()),
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
                        self.effect = None;
                        self.leave_direct();
                        if let Some(kb) = self.kb.as_mut()
                            && let Err(e) = kb.set_hw_mode(&m) {
                                self.disconnect(e.to_string());
                            }
                    }
                    Cmd::Software(i) => {
                        self.effect = EFFECTS.get(i).map(|e| (e.build)());
                        self.effect_start = Instant::now();
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
        match Keyboard::open().and_then(|mut kb| kb.read_hw_mode().map(|m| (kb, m))) {
            Ok((mut kb, current)) => {
                // Se o app fechou no meio de um efeito de software, o teclado
                // fica no modo Custom (apagado). Restauramos o efeito salvo.
                if current.mode == MODE_CUSTOM {
                    let _ = kb.set_hw_mode(&self.hw);
                } else {
                    self.hw = current;
                }
                let mut sh = self.shared.lock().unwrap();
                sh.device_hw = Some(self.hw);
                sh.status = Status::Connected;
                drop(sh);
                self.kb = Some(kb);
                self.in_direct = false;
            }
            Err(e) => self.shared.lock().unwrap().status = Status::Connecting(e.to_string()),
        }
    }

    fn disconnect(&mut self, reason: String) {
        self.kb = None;
        self.in_direct = false;
        self.last_attempt = Some(Instant::now());
        self.shared.lock().unwrap().status = Status::Connecting(format!("conexão perdida: {reason}"));
    }

    fn leave_direct(&mut self) {
        if !self.in_direct {
            return;
        }
        self.in_direct = false;
        if let Some(kb) = self.kb.as_mut() {
            let _ = kb.end_direct();
            let _ = kb.set_hw_mode(&self.hw);
        }
    }

    fn tick(&mut self, dt: f32) {
        // Pega os eventos de tecla e o estado compartilhado de uma vez só,
        // para segurar os locks o mínimo possível.
        let (raw_events, pressed_codes) = {
            let mut input = self.input.lock().unwrap();
            (std::mem::take(&mut input.engine_events), input.pressed.clone())
        };
        let (params, key_to_led, calibrate) = {
            let sh = self.shared.lock().unwrap();
            (sh.params.clone(), sh.key_to_led.clone(), sh.calibrate_led)
        };
        let events: Vec<KeyEvent> = raw_events
            .iter()
            .map(|&(code, pressed)| KeyEvent { code, led: key_to_led.get(&code).copied(), pressed })
            .collect();
        let pressed: Vec<usize> = pressed_codes.iter().filter_map(|c| key_to_led.get(c).copied()).collect();

        let mut frame: Frame = [[0; 3]; LEDS];
        let software = if let Some(led) = calibrate {
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
            // Prévia aproximada do efeito de fábrica.
            let k = self.hw.brightness as f32 / 4.0;
            let color = if self.hw.random { [200, 200, 200] } else { self.hw.color };
            let dim = if self.hw.mode == MODE_STATIC { 1.0 } else { 0.6 };
            frame.fill(crate::effects::scale(color, k * dim));
        }
        self.shared.lock().unwrap().preview = frame;
    }

    fn send(&mut self, frame: &Frame) -> std::io::Result<()> {
        let Some(kb) = self.kb.as_mut() else { return Ok(()) };
        if !self.in_direct {
            // O modo direto só fica estável com o firmware no modo Custom.
            kb.set_hw_mode(&HwMode { mode: MODE_CUSTOM, brightness: 4, ..self.hw })?;
            self.in_direct = true;
            self.last_sent = None;
        }
        if let Some((last, at)) = &self.last_sent
            && last == frame
            && at.elapsed() < KEEPALIVE_TIME
        {
            return Ok(());
        }
        kb.send_frame(frame)?;
        self.last_sent = Some((*frame, Instant::now()));
        Ok(())
    }
}
