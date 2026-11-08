//! keyglow: controle de RGB para o teclado SONiX/EVision 320f:5016.
//!
//! Organização:
//! - `device`  - protocolo USB do teclado
//! - `effects` - efeitos de software (crie os seus aqui!)
//! - `engine`  - thread que roda os efeitos e conversa com o teclado
//! - `input`   - leitura das teclas (evdev)
//! - `audio`   - captura do som do PC e FFT
//! - `config`  - configuração salva em disco
//! - `app`     - interface gráfica (egui)

mod app;
mod audio;
mod config;
mod device;
mod effects;
mod engine;
mod input;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn main() -> eframe::Result {
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

    let shared = Arc::new(Mutex::new(engine::Shared::new(config.params.clone(), config.key_to_led.clone())));
    let (tx, handle) = engine::spawn(
        Arc::clone(&shared),
        Arc::clone(&input),
        audio,
        audio_wanted,
        config.hw,
        config.software_effect,
    );

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("keyglow")
            .with_app_id("keyglow")
            .with_inner_size([1280.0, 640.0])
            .with_min_inner_size([900.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "keyglow",
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(shared, input, tx, handle, config)))),
    )
}
