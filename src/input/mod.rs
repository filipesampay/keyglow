//! Leitura das teclas, com uma implementação por sistema:
//! - Linux: evdev, só os dispositivos deste teclado (`linux.rs`);
//! - Windows: hook global de teclado (`windows.rs`).
//!
//! Os códigos de tecla são sempre os do Linux (evdev), inclusive no Windows,
//! para que a calibração salva valha nos dois sistemas.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// Este teclado manda `KEY_UNKNOWN` (240) sem parar pela segunda interface (Linux).
pub const KEY_PHANTOM: u16 = 240;

#[derive(Default)]
pub struct InputState {
    /// Códigos das teclas pressionadas agora.
    pub pressed: BTreeSet<u16>,
    /// Eventos (código, pressionada?) ainda não consumidos pelo motor de efeitos.
    pub engine_events: Vec<(u16, bool)>,
    /// Eventos ainda não consumidos pela interface (usada na calibração).
    pub gui_events: Vec<(u16, bool)>,
    /// Quantas fontes de teclas estão ativas.
    pub devices: usize,
    pub error: Option<String>,
}

impl InputState {
    /// Registra uma tecla pressionada (`down`) ou solta. Ignora repetições.
    pub fn record(&mut self, code: u16, down: bool) {
        if code == KEY_PHANTOM {
            return;
        }
        let changed = if down { self.pressed.insert(code) } else { self.pressed.remove(&code) };
        if !changed {
            return;
        }
        for queue in [&mut self.engine_events, &mut self.gui_events] {
            // Evita crescer sem limite se ninguém consumir (ex.: janela minimizada).
            if queue.len() >= 256 {
                queue.remove(0);
            }
            queue.push((code, down));
        }
    }
}

pub fn spawn(state: Arc<Mutex<InputState>>) {
    #[cfg(target_os = "linux")]
    linux::spawn(state);
    #[cfg(windows)]
    windows::spawn(state);
}

/// Nome curto de uma tecla (código evdev) para mostrar na tela.
pub fn key_name(code: u16) -> String {
    let name = match code {
        1 => "Esc",
        2..=10 => return (code - 1).to_string(),
        11 => "0",
        12 => "-",
        13 => "=",
        14 => "Backspace",
        15 => "Tab",
        16 => "Q",
        17 => "W",
        18 => "E",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        26 => "[",
        27 => "]",
        28 => "Enter",
        29 | 97 => "Ctrl",
        30 => "A",
        31 => "S",
        32 => "D",
        33 => "F",
        34 => "G",
        35 => "H",
        36 => "J",
        37 => "K",
        38 => "L",
        39 => ";",
        40 => "'",
        41 => "`",
        42 | 54 => "Shift",
        43 => "\\",
        44 => "Z",
        45 => "X",
        46 => "C",
        47 => "V",
        48 => "B",
        49 => "N",
        50 => "M",
        51 => ",",
        52 => ".",
        53 => "/",
        55 => "*",
        56 => "Alt",
        57 => "Espaço",
        58 => "Caps",
        59..=68 => return format!("F{}", code - 58),
        69 => "Num",
        70 => "ScrLk",
        71 => "7",
        72 => "8",
        73 => "9",
        74 => "-",
        75 => "4",
        76 => "5",
        77 => "6",
        78 => "+",
        79 => "1",
        80 => "2",
        81 => "3",
        82 => "0",
        83 => ",",
        86 => "\\|",
        87 => "F11",
        88 => "F12",
        89 => "/?",
        96 => "Enter",
        98 => "/",
        99 => "PrtSc",
        100 => "AltGr",
        102 => "Home",
        103 => "↑",
        104 => "PgUp",
        105 => "←",
        106 => "→",
        107 => "End",
        108 => "↓",
        109 => "PgDn",
        110 => "Ins",
        111 => "Del",
        113 => "Mudo",
        114 => "Vol-",
        115 => "Vol+",
        119 => "Pause",
        121 => ".",
        125 | 126 => "Super",
        127 => "Menu",
        _ => return format!("#{code}"),
    };
    name.to_string()
}
