//! Lê as teclas do teclado direto do kernel (evdev). Funciona mesmo com a
//! janela do app fora de foco. Precisa de permissão de leitura em
//! `/dev/input/event*` (veja `udev/70-keyglow.rules`).

use crate::device::{PID, VID};
use evdev::{Device, EventType, KeyCode};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

/// Este teclado manda `KEY_UNKNOWN` (240) sem parar pela segunda interface.
pub const KEY_PHANTOM: u16 = 240;

#[derive(Default)]
pub struct InputState {
    /// Códigos das teclas pressionadas agora.
    pub pressed: BTreeSet<u16>,
    /// Eventos (código, pressionada?) ainda não consumidos pelo motor de efeitos.
    pub engine_events: Vec<(u16, bool)>,
    /// Eventos ainda não consumidos pela interface (usada na calibração).
    pub gui_events: Vec<(u16, bool)>,
    /// Quantos dispositivos de entrada do teclado estão sendo lidos.
    pub devices: usize,
    pub error: Option<String>,
}

/// Nome curto de uma tecla para mostrar na tela.
pub fn key_name(code: u16) -> String {
    let full = format!("{:?}", KeyCode::new(code));
    let name = full.strip_prefix("KEY_").unwrap_or(&full);
    let short = match name {
        "ESC" => "Esc",
        "BACKSPACE" => "⌫",
        "ENTER" | "KPENTER" => "⏎",
        "LEFTSHIFT" | "RIGHTSHIFT" => "Shift",
        "LEFTCTRL" | "RIGHTCTRL" => "Ctrl",
        "LEFTALT" => "Alt",
        "RIGHTALT" => "AltGr",
        "LEFTMETA" | "RIGHTMETA" => "Super",
        "CAPSLOCK" => "Caps",
        "SPACE" => "Espaço",
        "TAB" => "Tab",
        "SYSRQ" => "PrtSc",
        "SCROLLLOCK" => "ScrLk",
        "PAUSE" => "Pause",
        "INSERT" => "Ins",
        "DELETE" => "Del",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" => "PgUp",
        "PAGEDOWN" => "PgDn",
        "NUMLOCK" => "Num",
        "UP" => "↑",
        "DOWN" => "↓",
        "LEFT" => "←",
        "RIGHT" => "→",
        "MINUS" => "-",
        "EQUAL" => "=",
        "LEFTBRACE" => "[",
        "RIGHTBRACE" => "]",
        "SEMICOLON" => ";",
        "APOSTROPHE" => "'",
        "GRAVE" => "`",
        "BACKSLASH" => "\\",
        "COMMA" => ",",
        "DOT" => ".",
        "SLASH" => "/",
        "102ND" => "\\|",
        "RO" => "/?",
        "COMPOSE" => "Menu",
        "KPSLASH" => "/",
        "KPASTERISK" => "*",
        "KPMINUS" => "-",
        "KPPLUS" => "+",
        "KPDOT" | "KPCOMMA" => ",",
        other => return other.strip_prefix("KP").map(|n| n.to_string()).unwrap_or_else(|| other.to_string()),
    };
    short.to_string()
}

/// Encontra os `/dev/input/event*` do teclado pelo vendor/product no sysfs.
fn keyboard_event_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/input") else { return paths };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("event") {
            continue;
        }
        let id = entry.path().join("device/id");
        let read_hex = |f: &str| {
            fs::read_to_string(id.join(f)).ok().and_then(|s| u16::from_str_radix(s.trim(), 16).ok())
        };
        if read_hex("vendor") == Some(VID) && read_hex("product") == Some(PID) {
            paths.push(PathBuf::from("/dev/input").join(name));
        }
    }
    paths.sort();
    paths
}

pub fn spawn(state: Arc<Mutex<InputState>>) {
    let mut errors = Vec::new();
    for path in keyboard_event_paths() {
        let mut dev = match Device::open(&path) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        // Só interessam as interfaces que têm teclas de verdade.
        if !dev.supported_keys().is_some_and(|k| k.contains(KeyCode::KEY_A) || k.contains(KeyCode::KEY_VOLUMEUP)) {
            continue;
        }
        state.lock().unwrap().devices += 1;

        let state = Arc::clone(&state);
        thread::Builder::new()
            .name(format!("input-{}", path.display()))
            .spawn(move || loop {
                match dev.fetch_events() {
                    Ok(events) => {
                        let mut guard = state.lock().unwrap();
                        let st = &mut *guard;
                        for ev in events {
                            // value: 1 = pressionou, 0 = soltou, 2 = repetição
                            if ev.event_type() != EventType::KEY || ev.value() == 2 || ev.code() == KEY_PHANTOM {
                                continue;
                            }
                            let (code, down) = (ev.code(), ev.value() == 1);
                            if down {
                                st.pressed.insert(code);
                            } else {
                                st.pressed.remove(&code);
                            }
                            for queue in [&mut st.engine_events, &mut st.gui_events] {
                                // Evita crescer sem limite se ninguém consumir
                                // (ex.: janela minimizada).
                                if queue.len() >= 256 {
                                    queue.remove(0);
                                }
                                queue.push((code, down));
                            }
                        }
                    }
                    Err(e) => {
                        let mut st = state.lock().unwrap();
                        st.devices -= 1;
                        st.error = Some(format!("leitura das teclas parou: {e}"));
                        return;
                    }
                }
            })
            .expect("não consegui criar a thread de entrada");
    }

    let mut st = state.lock().unwrap();
    if st.devices == 0 {
        st.error = Some(if errors.is_empty() {
            "não encontrei o teclado em /dev/input".into()
        } else {
            format!("sem permissão para ler as teclas ({}). Instale a regra udev.", errors.join("; "))
        });
    }
}
