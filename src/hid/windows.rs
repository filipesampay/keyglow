//! Windows: API HID do sistema via `hidapi` (backend nativo, sem C).
//!
//! No Windows, cada "top-level collection" vira um dispositivo separado,
//! então abrimos só a coleção de controle (usage page 0xFF1C); o teclado
//! em si continua com o Windows.

use super::CONTROL_USAGE_PAGE;
use crate::device::{PID, VID};
use hidapi::{HidApi, HidDevice};
use std::io;

pub struct Port {
    dev: HidDevice,
}

fn to_io(e: hidapi::HidError) -> io::Error {
    io::Error::other(e.to_string())
}

impl Port {
    pub fn open() -> io::Result<Self> {
        let api = HidApi::new().map_err(|e| io::Error::other(format!("não consegui listar os dispositivos USB ({e})")))?;
        let info = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == CONTROL_USAGE_PAGE)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "teclado 320f:5016 não encontrado"))?;
        let dev = info.open_device(&api).map_err(|e| {
            io::Error::other(format!("não consegui abrir o teclado ({e}); feche o software do fabricante, se estiver aberto"))
        })?;
        Ok(Self { dev })
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.dev.write(buf).map(|_| ()).map_err(to_io)
    }

    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
        self.dev.read_timeout(buf, timeout_ms).map_err(to_io)
    }
}
