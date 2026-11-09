//! Drivers de iluminação e busca automática de teclados.
//!
//! Cada tipo de teclado tem um driver:
//! - `evision`: firmware SONiX/EVision V2 (protocolo de fabricante, 0xFF1C);
//! - `lamparray`: padrão HID LampArray (Iluminação Dinâmica do Windows 11);
//! - `openrgb`: qualquer teclado que o OpenRGB controla, pelo servidor SDK dele.
//!
//! O app enxerga todos do mesmo jeito: uma **grade** de `rows x cols` células
//! (cada célula, uma posição de LED) e o trait [`Driver`].
//!
//! # Regra de segurança da busca
//!
//! [`scan`] **nunca escreve nada em dispositivo nenhum**: só lista as
//! interfaces HID (com o descriptor que o sistema já leu) e pergunta ao
//! servidor do OpenRGB quais teclados ele tem. Firmware de teclado às vezes
//! interpreta comandos desconhecidos como gravação de configuração, então
//! nenhum comando é mandado para "testar" um dispositivo. O primeiro contato
//! acontece em [`open`], e só com dispositivos que a busca reconheceu pelo
//! ID USB + tipo de interface.

pub mod evision;
pub mod lamparray;
pub mod openrgb;

use crate::hid::HidInfo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

pub type Rgb = [u8; 3];

/// Grade de LEDs do teclado, linha por linha.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    /// `false` = célula sem LED (o driver sabe que não há nada ali).
    pub present: Vec<bool>,
}

impl Grid {
    pub fn full(rows: usize, cols: usize) -> Self {
        Self { rows, cols, present: vec![true; rows * cols] }
    }

    pub fn len(&self) -> usize {
        self.rows * self.cols
    }

    pub fn row_col(&self, i: usize) -> (usize, usize) {
        (i / self.cols, i % self.cols)
    }

    pub fn has_led(&self, i: usize) -> bool {
        self.present.get(i).copied().unwrap_or(false)
    }
}

impl Default for Grid {
    /// Grade do teclado de referência (EVision 6 x 21).
    fn default() -> Self {
        Self::full(6, 21)
    }
}

/// Configuração de um efeito de fábrica (fica gravada no teclado).
/// Só os teclados EVision têm; os números de modo são os do 320f:5016.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HwMode {
    pub mode: u8,
    /// 0 (apagado) a 4 (máximo).
    pub brightness: u8,
    /// 0 (mais rápido) a 5 (mais lento).
    pub speed: u8,
    pub direction: u8,
    pub random: bool,
    pub color: Rgb,
}

impl Default for HwMode {
    fn default() -> Self {
        Self { mode: evision::MODE_STATIC, brightness: 4, speed: 3, direction: 0, random: false, color: [255, 0, 0] }
    }
}

pub trait Driver: Send {
    fn grid(&self) -> &Grid;

    /// Passa a mostrar as cores mandadas pelo app.
    fn enter_direct(&mut self, hw: &HwMode) -> io::Result<()>;

    /// Envia uma imagem completa (uma cor por célula da grade).
    fn send_frame(&mut self, frame: &[Rgb]) -> io::Result<()>;

    /// Devolve o controle ao teclado: efeito de fábrica `hw` (EVision) ou o
    /// efeito próprio do dispositivo (LampArray, OpenRGB).
    fn leave_direct(&mut self, hw: &HwMode) -> io::Result<()>;

    /// Ao conectar: lê o efeito de fábrica atual. Se o app tinha deixado o
    /// teclado no modo direto (fechou no meio), restaura `saved`. Devolve o
    /// efeito em vigor, ou `None` se o teclado não tem efeitos de fábrica.
    fn connect(&mut self, _saved: &HwMode) -> io::Result<Option<HwMode>> {
        Ok(None)
    }

    /// Efeitos de fábrica (valor, nome). Vazio = o teclado não tem.
    fn hw_modes(&self) -> &'static [(u8, &'static str)] {
        &[]
    }

    /// Grava um efeito de fábrica.
    fn set_hw_mode(&mut self, _m: &HwMode) -> io::Result<()> {
        Ok(())
    }

    /// Reenvia a mesma imagem neste intervalo (alguns firmwares apagam sem isso).
    fn keepalive(&self) -> Option<Duration> {
        None
    }

    /// Mapa tecla (evdev) -> célula conhecido sem calibração.
    fn default_key_map(&self) -> BTreeMap<u16, usize> {
        BTreeMap::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Evision,
    LampArray,
    OpenRgb,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Evision => "EVision",
            Kind::LampArray => "LampArray",
            Kind::OpenRgb => "via OpenRGB",
        }
    }
}

/// Teclado encontrado pela busca.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceEntry {
    /// Identificador estável (a calibração é salva por ele).
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Já foi testado com o keyglow num teclado de verdade?
    pub tested: bool,
    pub(crate) source: Source,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Hid(HidInfo),
    OpenRgb { name: String, location: String },
}

/// Procura teclados compatíveis. Não escreve em nenhum dispositivo (veja o
/// comentário do módulo).
pub fn scan() -> (Vec<DeviceEntry>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    match crate::hid::enumerate() {
        Ok(list) => {
            for info in list {
                if let Some(e) = evision::recognize(&info).or_else(|| lamparray::recognize(&info))
                    && !found.iter().any(|f: &DeviceEntry| f.id == e.id)
                {
                    found.push(e);
                }
            }
        }
        Err(e) => errors.push(e.to_string()),
    }
    // OpenRGB é opcional: sem o servidor rodando, não é erro. Teclados que
    // já achamos direto não entram de novo pelo OpenRGB.
    let direct: Vec<String> = found
        .iter()
        .filter_map(|e| match &e.source {
            Source::Hid(info) => Some(openrgb::hid_location(&info.path)),
            _ => None,
        })
        .collect();
    found.extend(openrgb::scan().into_iter().filter(|e| match &e.source {
        Source::OpenRgb { location, .. } => !direct.contains(location),
        _ => true,
    }));
    (found, errors)
}

/// Abre o teclado. É aqui (e não na busca) que o primeiro comando é enviado.
pub fn open(entry: &DeviceEntry) -> io::Result<Box<dyn Driver>> {
    match (&entry.kind, &entry.source) {
        (Kind::Evision, Source::Hid(info)) => Ok(Box::new(evision::Evision::open(info)?)),
        (Kind::LampArray, Source::Hid(info)) => Ok(Box::new(lamparray::LampArray::open(info)?)),
        (Kind::OpenRgb, Source::OpenRgb { name, location }) => Ok(Box::new(openrgb::OpenRgb::open(name, location)?)),
        _ => Err(io::Error::other("dispositivo inválido")),
    }
}
