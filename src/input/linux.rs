//! Linux: lê os `/dev/input/event*` deste teclado via evdev. Funciona com a
//! janela fora de foco e não enxerga outros teclados. Precisa da regra udev
//! (`udev/70-keyglow.rules`).

use super::InputState;
use crate::device::{PID, VID};
use evdev::{Device, EventType, KeyCode};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

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
        let read_hex = |f: &str| fs::read_to_string(id.join(f)).ok().and_then(|s| u16::from_str_radix(s.trim(), 16).ok());
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
                        let mut st = state.lock().unwrap();
                        for ev in events {
                            // value: 1 = pressionou, 0 = soltou, 2 = repetição
                            if ev.event_type() == EventType::KEY && ev.value() != 2 {
                                st.record(ev.code(), ev.value() == 1);
                            }
                        }
                    }
                    Err(e) => {
                        let mut st = state.lock().unwrap();
                        st.devices -= 1;
                        st.error = Some(format!("a leitura das teclas parou: {e}"));
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
            format!("sem permissão para ler as teclas ({}). Instale a regra udev do README.", errors.join("; "))
        });
    }
}
