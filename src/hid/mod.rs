//! Acesso HID aos dispositivos, uma implementação por sistema:
//! - Linux: `/dev/hidraw*` direto (`linux.rs`);
//! - Windows e macOS: biblioteca `hidapi` (`native.rs`).
//!
//! Todas expõem a mesma API:
//! - [`enumerate`] lista as interfaces HID (uma entrada por coleção de topo),
//!   **sem abrir nem escrever em nada**;
//! - `Port::open(&info)` abre uma delas;
//! - [`HidIo`] lê e escreve relatórios (o report ID vai no byte 0).

pub mod descriptor;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{enumerate, Port};

#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(any(windows, target_os = "macos"))]
pub use native::{enumerate, Port};

use std::io;

/// Uma coleção de topo de um dispositivo HID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidInfo {
    /// Caminho para abrir (`/dev/hidraw3`, caminho do hidapi...).
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub product: String,
    /// Versão do dispositivo informada pelo USB (bcdDevice); 0 = desconhecida.
    /// Nos teclados EVision é a versão do firmware.
    pub release: u16,
}

/// Canal de relatórios HID. É um trait para os drivers poderem ser testados
/// com um dispositivo de mentira.
pub trait HidIo: Send {
    /// Envia um relatório de saída (byte 0 = report ID).
    fn write(&mut self, buf: &[u8]) -> io::Result<()>;
    /// Lê um relatório de entrada; `Ok(0)` = tempo esgotado.
    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize>;
    /// Lê um relatório de feature. `buf[0]` = report ID; devolve os bytes lidos.
    fn get_feature(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "relatórios de feature não suportados"))
    }
    /// Envia um relatório de feature (byte 0 = report ID).
    fn send_feature(&mut self, _buf: &[u8]) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "relatórios de feature não suportados"))
    }
    /// Report descriptor do dispositivo.
    fn report_descriptor(&mut self) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "report descriptor indisponível"))
    }
}

impl HidIo for Port {
    fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        Port::write(self, buf)
    }
    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
        Port::read_timeout(self, buf, timeout_ms)
    }
    fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Port::get_feature(self, buf)
    }
    fn send_feature(&mut self, buf: &[u8]) -> io::Result<()> {
        Port::send_feature(self, buf)
    }
    fn report_descriptor(&mut self) -> io::Result<Vec<u8>> {
        Port::report_descriptor(self)
    }
}

#[cfg(test)]
pub mod mock;
