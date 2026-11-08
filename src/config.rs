//! Configuração salva em `~/.config/keyglow/config.json`.

use crate::device::HwMode;
use crate::effects::Params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Código da tecla (evdev) -> índice do LED, preenchido na calibração.
    pub key_to_led: BTreeMap<u16, usize>,
    /// Último efeito de fábrica usado. Ele é restaurado ao fechar o app.
    pub hw: HwMode,
    pub params: Params,
    /// `Some(i)` = efeito de software `i` ativo; `None` = efeito de fábrica.
    pub software_effect: Option<usize>,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("keyglow").join("config.json"))
}

impl Config {
    pub fn load() -> Self {
        let Some(p) = path() else { return Self::default() };
        fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = fs::write(&p, json) {
                    eprintln!("não consegui salvar {}: {e}", p.display());
                }
            }
            Err(e) => eprintln!("erro ao gerar a configuração: {e}"),
        }
    }

    /// LED -> código da tecla (o inverso de `key_to_led`).
    pub fn led_to_key(&self) -> BTreeMap<usize, u16> {
        self.key_to_led.iter().map(|(&k, &l)| (l, k)).collect()
    }
}
