//! Modo de captura para desenvolvimento: `KEYGLOW_SCREENSHOT=saida.png keyglow`
//! abre o app, espera a interface estabilizar, salva um PNG só da janela
//! do app e fecha.

use eframe::egui::{self, ScreenshotCallback, ViewportCommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Quadros de espera antes da captura (conexão com o teclado, áudio etc.).
const WARMUP_FRAMES: u32 = 90;

pub struct Screenshot {
    path: PathBuf,
    frames: u32,
    requested: bool,
    done: Arc<AtomicBool>,
}

impl Screenshot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("KEYGLOW_SCREENSHOT")?;
        Some(Self { path: path.into(), frames: 0, requested: false, done: Arc::new(AtomicBool::new(false)) })
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.done.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        self.frames += 1;
        ctx.request_repaint();
        if self.requested || self.frames < WARMUP_FRAMES {
            return;
        }
        self.requested = true;
        let (path, done) = (self.path.clone(), Arc::clone(&self.done));
        let ctx2 = ctx.clone();
        ctx.send_viewport_cmd(ViewportCommand::Screenshot(ScreenshotCallback::new(move |img| {
            let [w, h] = img.size;
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::save_buffer(&path, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                Ok(()) => eprintln!("screenshot salvo em {}", path.display()),
                Err(e) => eprintln!("falha ao salvar screenshot: {e}"),
            }
            done.store(true, Ordering::Relaxed);
            ctx2.request_repaint();
        })));
    }
}
