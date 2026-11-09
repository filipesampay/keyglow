//! Linux: lê os `/dev/input/event*` do teclado conectado via evdev. Funciona
//! com a janela fora de foco e não enxerga outros teclados. Precisa da regra
//! udev (`udev/70-keyglow.rules`).
//!
//! O teclado pode mudar (o usuário escolhe outro na interface), então uma
//! thread vigia `InputState::target` e reabre os dispositivos quando ele muda.

use super::InputState;
use evdev::{Device, EventType, KeyCode};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Encontra os `/dev/input/event*` com este vendor/product no sysfs.
fn event_paths(vid: u16, pid: u16) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/input") else { return paths };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("event") {
            continue;
        }
        let id = entry.path().join("device/id");
        let read_hex = |f: &str| fs::read_to_string(id.join(f)).ok().and_then(|s| u16::from_str_radix(s.trim(), 16).ok());
        if read_hex("vendor") == Some(vid) && read_hex("product") == Some(pid) {
            paths.push(PathBuf::from("/dev/input").join(name));
        }
    }
    paths.sort();
    paths
}

/// Incrementado a cada troca de teclado; leitores de uma geração antiga param.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn spawn(state: Arc<Mutex<InputState>>) {
    thread::Builder::new()
        .name("input-watch".into())
        .spawn(move || {
            let mut current: Option<(u16, u16)> = None;
            loop {
                let (target, devices) = {
                    let st = state.lock().unwrap();
                    (st.target, st.devices)
                };
                // Reabre se o teclado mudou ou se a leitura caiu.
                if target != current || (target.is_some() && devices == 0) {
                    current = target;
                    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                    {
                        let mut st = state.lock().unwrap();
                        st.devices = 0;
                        st.pressed.clear();
                        if target.is_none() {
                            st.error = None;
                        }
                    }
                    if let Some((vid, pid)) = target {
                        open_all(&state, vid, pid, generation);
                    }
                }
                thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("não consegui criar a thread de entrada");
}

fn open_all(state: &Arc<Mutex<InputState>>, vid: u16, pid: u16, generation: u64) {
    let mut errors = Vec::new();
    for path in event_paths(vid, pid) {
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

        let state = Arc::clone(state);
        thread::Builder::new()
            .name(format!("input-{}", path.display()))
            .spawn(move || loop {
                let result = dev.fetch_events().map(|evs| evs.collect::<Vec<_>>());
                if GENERATION.load(Ordering::SeqCst) != generation {
                    return; // outro teclado foi escolhido
                }
                let mut st = state.lock().unwrap();
                match result {
                    Ok(events) => {
                        for ev in events {
                            // value: 1 = pressionou, 0 = soltou, 2 = repetição
                            if ev.event_type() == EventType::KEY && ev.value() != 2 {
                                st.record(ev.code(), ev.value() == 1);
                            }
                        }
                    }
                    Err(e) => {
                        st.devices = st.devices.saturating_sub(1);
                        st.error = Some(format!("a leitura das teclas parou: {e}"));
                        return;
                    }
                }
            })
            .expect("não consegui criar a thread de entrada");
    }

    let mut st = state.lock().unwrap();
    if st.devices > 0 {
        st.error = None;
    } else {
        st.error = Some(if errors.is_empty() {
            "não encontrei as teclas deste teclado em /dev/input".into()
        } else {
            format!("sem permissão para ler as teclas ({}). Instale a regra udev do README.", errors.join("; "))
        });
    }
}
