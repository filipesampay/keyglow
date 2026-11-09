//! keyglow: controle de RGB para teclados, com busca automática.
//! Roda no Linux, no Windows e no macOS.
//!
//! Organização:
//! - `drivers` - busca de teclados e um driver por tipo (EVision, LampArray, OpenRGB)
//! - `effects` - efeitos de software (crie os seus aqui!)
//! - `engine`  - thread que roda os efeitos e conversa com o teclado
//! - `hid`     - acesso HID (hidraw no Linux, hidapi no Windows e no macOS)
//! - `keymap`  - conversão de códigos de tecla (HID, OpenRGB) para evdev
//! - `input`   - leitura das teclas (evdev no Linux, hook no Windows)
//! - `audio`   - captura do som do PC (PipeWire / WASAPI) e FFT
//! - `config`  - configuração salva em disco
//! - `app`     - interface gráfica (egui)

// No Windows, a versão release não abre uma janela de console junto.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod default_layout;
mod drivers;
mod effects;
mod engine;
mod hid;
mod input;
mod keyboard_view;
mod keymap;
mod onboard;
mod screenshot;
mod theme;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// `keyglow --scan`: lista as interfaces HID e os teclados reconhecidos,
/// sem abrir a janela e sem enviar nada a nenhum dispositivo.
fn print_scan() {
    match hid::enumerate() {
        Ok(list) => {
            println!("Interfaces HID ({}):", list.len());
            for i in list {
                println!("  {:04x}:{:04x}  página {:04x} uso {:04x}  {}", i.vid, i.pid, i.usage_page, i.usage, i.product);
            }
        }
        Err(e) => println!("não consegui listar as interfaces HID: {e}"),
    }
    let (found, errors) = drivers::scan();
    println!("\nTeclados compatíveis ({}):", found.len());
    for d in &found {
        println!("  {}  [{}{}]  id {}", d.name, d.kind.label(), if d.tested { "" } else { ", não testado" }, d.id);
    }
    for e in errors {
        println!("  erro: {e}");
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--scan") {
        print_scan();
        return Ok(());
    }

    // O MangoHud está ligado no sistema todo (/etc/environment) e se desenharia
    // por cima da janela. Isto o desliga só para este app.
    // SAFETY: feito antes de criar qualquer thread.
    unsafe {
        std::env::set_var("DISABLE_MANGOHUD", "1");
        std::env::remove_var("MANGOHUD");
    }

    let config = config::Config::load();

    let input = Arc::new(Mutex::new(input::InputState::default()));
    input::spawn(Arc::clone(&input));

    let audio = Arc::new(Mutex::new(effects::AudioData::default()));
    let audio_wanted = Arc::new(AtomicBool::new(false));
    audio::spawn(Arc::clone(&audio), Arc::clone(&audio_wanted));

    let shared = Arc::new(Mutex::new(engine::Shared::new(config.params.clone())));
    let start = engine::Start { hw: config.hw, software_effect: config.software_effect, device: config.last_device.clone() };
    let (tx, handle) = engine::spawn(Arc::clone(&shared), Arc::clone(&input), audio, audio_wanted, start);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("keyglow")
            .with_app_id("keyglow")
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([1040.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "keyglow",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, shared, input, tx, handle, config)))),
    )
}
