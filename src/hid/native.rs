//! Windows e macOS: API HID do sistema via `hidapi`.
//!
//! - Windows: backend nativo, sem C. Cada "top-level collection" vira um
//!   dispositivo separado, então abrimos só a coleção de controle.
//! - macOS: IOKit, aberto sem exclusividade (feature `macos-shared-device`)
//!   para o teclado continuar digitando.

use super::HidInfo;
use hidapi::{HidApi, HidDevice};
use std::ffi::CString;
use std::io;

pub struct Port {
    dev: HidDevice,
}

fn to_io(e: hidapi::HidError) -> io::Error {
    io::Error::other(e.to_string())
}

/// Lista as coleções HID de todos os dispositivos. Não abre nenhum.
pub fn enumerate() -> io::Result<Vec<HidInfo>> {
    let api = HidApi::new().map_err(|e| io::Error::other(format!("não consegui listar os dispositivos USB ({e})")))?;
    Ok(api
        .device_list()
        .map(|d| HidInfo {
            path: d.path().to_string_lossy().into_owned(),
            vid: d.vendor_id(),
            pid: d.product_id(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            product: d.product_string().unwrap_or_default().trim().to_string(),
            release: d.release_number(),
        })
        .collect())
}

impl Port {
    pub fn open(info: &HidInfo) -> io::Result<Self> {
        let api = HidApi::new().map_err(|e| io::Error::other(format!("não consegui listar os dispositivos USB ({e})")))?;
        let path = CString::new(info.path.clone()).map_err(io::Error::other)?;
        let dev = api.open_path(&path).map_err(|e| {
            io::Error::other(format!("não consegui abrir {} ({e}); feche o software do fabricante, se estiver aberto", info.product))
        })?;
        Ok(Self { dev })
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.dev.write(buf).map(|_| ()).map_err(to_io)
    }

    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
        self.dev.read_timeout(buf, timeout_ms).map_err(to_io)
    }

    pub fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.dev.get_feature_report(buf).map_err(to_io)
    }

    pub fn send_feature(&mut self, buf: &[u8]) -> io::Result<()> {
        self.dev.send_feature_report(buf).map_err(to_io)
    }

    pub fn report_descriptor(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; 4096];
        let n = self.dev.get_report_descriptor(&mut buf).map_err(to_io)?;
        buf.truncate(n);
        Ok(buf)
    }
}
