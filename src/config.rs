//! Configuração salva em `~/.config/keyglow/config.json` (Linux),
//! `%APPDATA%\keyglow\config.json` (Windows) ou
//! `~/Library/Application Support/keyglow/config.json` (macOS).

use crate::drivers::{HwMode, Rgb};
use crate::effects::Params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// Id do teclado em que o keyglow nasceu; as configurações antigas eram dele.
pub const LEGACY_DEVICE: &str = "evision:320f:5016";

/// O que é salvo para cada teclado.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// Código da tecla (evdev) -> célula da grade, preenchido na calibração.
    pub key_to_led: BTreeMap<u16, usize>,
    /// Cores do efeito "Pintura".
    pub paint: Vec<Rgb>,
    /// Tabela de teclas como estava na primeira vez que o keyglow a leu
    /// (hexadecimal), para desfazer as mudanças.
    pub original_keys: String,
    /// Nomes das macros (o teclado só guarda os passos).
    pub macro_names: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Configuração por teclado (chave = `DeviceEntry::id`).
    pub devices: BTreeMap<String, DeviceConfig>,
    /// Último teclado escolhido.
    pub last_device: Option<String>,
    /// Último efeito de fábrica usado. Ele é restaurado ao fechar o app.
    pub hw: HwMode,
    pub params: Params,
    /// `Some(i)` = efeito de software `i` ativo; `None` = efeito de fábrica.
    pub software_effect: Option<usize>,
    /// Formato antigo (um teclado só); lido para migrar, nunca gravado.
    #[serde(skip_serializing)]
    key_to_led: BTreeMap<u16, usize>,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("keyglow").join("config.json"))
}

impl Config {
    pub fn load() -> Self {
        let config: Self = path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        config.migrated()
    }

    /// Move a calibração e a pintura do formato antigo para o 320f:5016.
    fn migrated(mut self) -> Self {
        let key_to_led = std::mem::take(&mut self.key_to_led);
        let paint = std::mem::take(&mut self.params.paint);
        if (!key_to_led.is_empty() || !paint.is_empty()) && !self.devices.contains_key(LEGACY_DEVICE) {
            self.devices.insert(LEGACY_DEVICE.to_string(), DeviceConfig { key_to_led, paint, ..Default::default() });
        }
        self
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_single_keyboard_config() {
        let old = r#"{"key_to_led": {"30": 64, "1": 0}, "params": {"paint": [[1,2,3]]}, "software_effect": 2}"#;
        let c = serde_json::from_str::<Config>(old).unwrap().migrated();
        let d = &c.devices[LEGACY_DEVICE];
        assert_eq!(d.key_to_led[&30], 64);
        assert_eq!(d.paint, vec![[1, 2, 3]]);
        assert_eq!(c.software_effect, Some(2));
        let json = serde_json::to_string(&c).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("key_to_led").is_none());
        assert!(v["params"].get("paint").is_none());
        // Gravado de novo e relido, continua igual.
        let again = serde_json::from_str::<Config>(&json).unwrap().migrated();
        assert_eq!(again.devices[LEGACY_DEVICE].key_to_led, d.key_to_led);
    }
}
