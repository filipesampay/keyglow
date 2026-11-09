//! Linux: `/dev/hidraw*` direto, sem bibliotecas externas.

use super::{descriptor, HidInfo};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub struct Port {
    file: File,
    /// `/sys/class/hidraw/hidrawN/device`, onde fica o report descriptor.
    sys: PathBuf,
}

/// Lê `HID_ID=0003:0000320F:00005016` e `HID_NAME=...` do uevent.
fn parse_uevent(text: &str) -> Option<(u16, u16, String)> {
    let mut ids = None;
    let mut name = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("HID_ID=") {
            let mut parts = v.split(':').skip(1);
            let vid = u32::from_str_radix(parts.next()?, 16).ok()? as u16;
            let pid = u32::from_str_radix(parts.next()?, 16).ok()? as u16;
            ids = Some((vid, pid));
        } else if let Some(v) = line.strip_prefix("HID_NAME=") {
            name = v.trim().to_string();
        }
    }
    ids.map(|(v, p)| (v, p, name))
}

/// Lista as coleções HID de todos os hidraw, lendo só o sysfs (não abre
/// nenhum dispositivo).
pub fn enumerate() -> io::Result<Vec<HidInfo>> {
    let mut out = Vec::new();
    let mut entries: Vec<_> = fs::read_dir("/sys/class/hidraw")?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let dev = entry.path().join("device");
        let Some((vid, pid, product)) = fs::read_to_string(dev.join("uevent")).ok().and_then(|t| parse_uevent(&t)) else {
            continue;
        };
        let desc = fs::read(dev.join("report_descriptor")).unwrap_or_default();
        // `device` é a interface HID; dois níveis acima fica o dispositivo USB.
        let release = fs::read_to_string(dev.join("../../bcdDevice"))
            .ok()
            .and_then(|t| u16::from_str_radix(t.trim(), 16).ok())
            .unwrap_or(0);
        let path = Path::new("/dev").join(entry.file_name()).to_string_lossy().into_owned();
        for (usage_page, usage) in descriptor::top_level_usages(&desc) {
            out.push(HidInfo { path: path.clone(), vid, pid, usage_page, usage, product: product.clone(), release });
        }
    }
    Ok(out)
}

/// `_IOC(_IOC_READ | _IOC_WRITE, 'H', nr, len)` do `linux/hidraw.h`.
fn hid_ioc(nr: u64, len: usize) -> u64 {
    (3 << 30) | ((len as u64) << 16) | ((b'H' as u64) << 8) | nr
}
const HIDIOCSFEATURE: u64 = 0x06;
const HIDIOCGFEATURE: u64 = 0x07;

impl Port {
    pub fn open(info: &HidInfo) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&info.path).map_err(|e| {
            io::Error::new(e.kind(), format!("sem acesso a {} ({e}); instale a regra udev do README", info.path))
        })?;
        let name = Path::new(&info.path).file_name().unwrap_or_default();
        Ok(Self { file, sys: Path::new("/sys/class/hidraw").join(name).join("device") })
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

    pub fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: o ioctl escreve no máximo `buf.len()` bytes em `buf`.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), hid_ioc(HIDIOCGFEATURE, buf.len()) as _, buf.as_mut_ptr()) };
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r as usize) }
    }

    pub fn send_feature(&mut self, buf: &[u8]) -> io::Result<()> {
        // SAFETY: o ioctl lê no máximo `buf.len()` bytes de `buf`.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), hid_ioc(HIDIOCSFEATURE, buf.len()) as _, buf.as_ptr()) };
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    pub fn report_descriptor(&mut self) -> io::Result<Vec<u8>> {
        fs::read(self.sys.join("report_descriptor"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uevent() {
        let text = "DRIVER=hid-generic\nHID_ID=0003:0000320F:00005016\nHID_NAME=SONiX Gaming Keyboard\n";
        assert_eq!(super::parse_uevent(text), Some((0x320F, 0x5016, "SONiX Gaming Keyboard".into())));
    }
}
