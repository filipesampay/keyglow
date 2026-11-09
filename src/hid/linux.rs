//! Linux: `/dev/hidraw*` direto, sem bibliotecas externas.

use super::CONTROL_USAGE_PAGE;
use crate::device::{PID, VID};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub struct Port {
    file: File,
}

impl Port {
    /// Procura o hidraw do teclado cujo report descriptor declara a usage page
    /// de controle (`06 1C FF`).
    fn find_path() -> io::Result<PathBuf> {
        let wanted = format!("HID_ID=0003:{:08X}:{:08X}", VID, PID);
        let [lo, hi] = CONTROL_USAGE_PAGE.to_le_bytes();
        for entry in fs::read_dir("/sys/class/hidraw")? {
            let entry = entry?;
            let dev = entry.path().join("device");
            let uevent = fs::read_to_string(dev.join("uevent")).unwrap_or_default();
            if !uevent.lines().any(|l| l == wanted) {
                continue;
            }
            let desc = fs::read(dev.join("report_descriptor")).unwrap_or_default();
            if desc.windows(3).any(|w| w == [0x06, lo, hi]) {
                return Ok(PathBuf::from("/dev").join(entry.file_name()));
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "teclado 320f:5016 não encontrado"))
    }

    pub fn open() -> io::Result<Self> {
        let path = Self::find_path()?;
        let file = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| {
            io::Error::new(e.kind(), format!("sem acesso a {} ({e}); instale a regra udev do README", path.display()))
        })?;
        Ok(Self { file })
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)
    }

    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
        let mut pfd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: `pfd` é um pollfd válido e vive durante toda a chamada.
        let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        if r == 0 {
            return Ok(0);
        }
        self.file.read(buf)
    }
}
