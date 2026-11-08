//! Comunicação com o teclado SONiX/EVision `320f:5016` via `/dev/hidraw`.
//!
//! Protocolo "EVision V2" (o mesmo que o OpenRGB documenta), com uma
//! diferença descoberta na prática: neste firmware os números dos modos
//! de fábrica são os do OpenRGB menos 1 (5 = fixo, 6 = reativo...).
//!
//! Formato de cada pacote (64 bytes):
//! `[0x04, chk_lo, chk_hi, cmd, tamanho, off_lo, off_hi, status, dados...]`
//! onde o checksum é a soma (u16) dos bytes 3..64.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub const VID: u16 = 0x320F;
pub const PID: u16 = 0x5016;

/// O teclado expõe uma matriz de 6 linhas x 21 colunas de LEDs.
pub const ROWS: usize = 6;
pub const COLS: usize = 21;
pub const LEDS: usize = ROWS * COLS;

pub type Rgb = [u8; 3];
/// Uma "imagem" completa do teclado: uma cor por LED, linha por linha.
pub type Frame = [Rgb; LEDS];

const REPORT_ID: u8 = 0x04;
const PACKET: usize = 64;
const MAX_PAYLOAD: usize = PACKET - 8;
const TIMEOUT_MS: i32 = 500;

mod cmd {
    pub const BEGIN: u8 = 0x01;
    pub const END: u8 = 0x02;
    pub const READ_CONFIG: u8 = 0x05;
    pub const WRITE_CONFIG: u8 = 0x06;
    /// Cores enviadas em tempo real pelo software.
    pub const DIRECT: u8 = 0x12;
    pub const END_DIRECT: u8 = 0x13;
}

const OFF_CURRENT_PROFILE: u16 = 0x00;
const OFF_FIRST_PROFILE: u16 = 0x01;
const PROFILE_STRIDE: u16 = 0x40;

/// Modo "Custom" do firmware. Com ele ativo, o modo direto fica estável.
pub const MODE_CUSTOM: u8 = 0x13;
pub const MODE_STATIC: u8 = 5;

/// Efeitos de fábrica (valor do firmware, nome). Os nomes vêm do OpenRGB;
/// confirmados na prática: 5, 6 e 7.
pub const HW_MODES: &[(u8, &str)] = &[
    (0, "Onda curta"),
    (1, "Onda longa"),
    (2, "Caminhos (zig-zag?)"),
    (3, "Espectro"),
    (4, "Respiração"),
    (5, "Fixo"),
    (6, "Reativo"),
    (7, "Ondulação ao digitar"),
    (8, "Linha reativa"),
    (9, "Estrelas rápidas"),
    (10, "Florescer"),
    (11, "Arco-íris vertical"),
    (12, "Furacão"),
    (13, "Acumular"),
    (14, "Estrelas lentas"),
    (15, "Vai e volta"),
    (16, "Surmount"),
    (17, "Círculo arco-íris"),
];

pub fn hw_mode_name(mode: u8) -> String {
    HW_MODES
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("Modo {mode}"))
}

/// Configuração de um efeito de fábrica (fica gravada no teclado).
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
        Self { mode: MODE_STATIC, brightness: 4, speed: 3, direction: 0, random: false, color: [255, 0, 0] }
    }
}

pub struct Keyboard {
    file: File,
}

impl Keyboard {
    /// Procura a interface de controle (usage page 0xFF1C) do teclado.
    pub fn find_path() -> io::Result<PathBuf> {
        let wanted = format!("HID_ID=0003:{:08X}:{:08X}", VID, PID);
        for entry in fs::read_dir("/sys/class/hidraw")? {
            let entry = entry?;
            let dev = entry.path().join("device");
            let uevent = fs::read_to_string(dev.join("uevent")).unwrap_or_default();
            if !uevent.lines().any(|l| l == wanted) {
                continue;
            }
            let desc = fs::read(dev.join("report_descriptor")).unwrap_or_default();
            if desc.windows(3).any(|w| w == [0x06, 0x1C, 0xFF]) {
                return Ok(PathBuf::from("/dev").join(entry.file_name()));
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "teclado 320f:5016 não encontrado"))
    }

    pub fn open() -> io::Result<Self> {
        let path = Self::find_path()?;
        let file = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| {
            io::Error::new(e.kind(), format!("sem acesso a {}: {e}", path.display()))
        })?;
        Ok(Self { file })
    }

    fn wait_readable(&self) -> io::Result<bool> {
        let mut pfd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: `pfd` é um pollfd válido e vive durante toda a chamada.
        let r = unsafe { libc::poll(&mut pfd, 1, TIMEOUT_MS) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(r > 0)
    }

    /// Envia um pacote e espera a resposta correspondente.
    fn query(&mut self, command: u8, offset: u16, data: &[u8], size: u8) -> io::Result<Vec<u8>> {
        debug_assert!(data.len() <= MAX_PAYLOAD);
        let mut buf = [0u8; PACKET];
        buf[0] = REPORT_ID;
        buf[3] = command;
        buf[4] = size;
        buf[5] = (offset & 0xFF) as u8;
        buf[6] = (offset >> 8) as u8;
        buf[8..8 + data.len()].copy_from_slice(data);
        let sum = buf[3..].iter().fold(0u16, |acc, &b| acc.wrapping_add(b as u16));
        buf[1] = (sum & 0xFF) as u8;
        buf[2] = (sum >> 8) as u8;

        self.file.write_all(&buf)?;

        // A mesma interface também manda eventos do botão de volume
        // (outros report IDs), então ignoramos o que não for resposta.
        let mut resp = [0u8; PACKET];
        loop {
            if !self.wait_readable()? {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "o teclado não respondeu"));
            }
            let n = self.file.read(&mut resp)?;
            if n == PACKET && resp[0] == REPORT_ID && resp[3] == command && resp[5] == buf[5] && resp[6] == buf[6] {
                break;
            }
        }
        if resp[7] != 0 {
            return Err(io::Error::other(format!("o teclado recusou o comando 0x{command:02x} (status {})", resp[7])));
        }
        let len = (resp[4] as usize).min(MAX_PAYLOAD);
        Ok(resp[8..8 + len].to_vec())
    }

    fn read(&mut self, command: u8, mut offset: u16, mut len: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        while len > 0 {
            let chunk = len.min(MAX_PAYLOAD);
            let data = self.query(command, offset, &[], chunk as u8)?;
            if data.len() < chunk {
                return Err(io::Error::other("resposta curta do teclado"));
            }
            out.extend_from_slice(&data[..chunk]);
            offset += chunk as u16;
            len -= chunk;
        }
        Ok(out)
    }

    fn write(&mut self, command: u8, mut offset: u16, data: &[u8]) -> io::Result<()> {
        for chunk in data.chunks(MAX_PAYLOAD) {
            self.query(command, offset, chunk, chunk.len() as u8)?;
            offset += chunk.len() as u16;
        }
        Ok(())
    }

    /// Lê o perfil ativo, corrigindo para 0 se estiver inválido.
    fn current_profile(&mut self) -> io::Result<u16> {
        let p = self.read(cmd::READ_CONFIG, OFF_CURRENT_PROFILE, 1)?[0];
        if p > 2 {
            self.write(cmd::WRITE_CONFIG, OFF_CURRENT_PROFILE, &[0])?;
            return Ok(0);
        }
        Ok(p as u16)
    }

    pub fn read_hw_mode(&mut self) -> io::Result<HwMode> {
        let profile = self.current_profile()?;
        let d = self.read(cmd::READ_CONFIG, OFF_FIRST_PROFILE + profile * PROFILE_STRIDE, 8)?;
        Ok(HwMode {
            mode: d[0],
            brightness: d[1],
            speed: d[2],
            direction: d[3],
            random: d[4] != 0,
            color: [d[5], d[6], d[7]],
        })
    }

    /// Grava um efeito de fábrica no perfil ativo.
    pub fn set_hw_mode(&mut self, m: &HwMode) -> io::Result<()> {
        self.query(cmd::BEGIN, 0, &[], 0)?;
        let result = self.write_hw_mode(m);
        self.query(cmd::END, 0, &[], 0)?;
        result
    }

    fn write_hw_mode(&mut self, m: &HwMode) -> io::Result<()> {
        let profile = self.current_profile()?;
        let data = [
            m.mode,
            m.brightness.min(4),
            m.speed.min(5),
            m.direction,
            if m.random { 0xFF } else { 0x00 },
            m.color[0],
            m.color[1],
            m.color[2],
        ];
        self.write(cmd::WRITE_CONFIG, OFF_FIRST_PROFILE + profile * PROFILE_STRIDE, &data)
    }

    /// Envia uma imagem completa. O teclado só mantém a imagem enquanto
    /// novas chegam (~30 por segundo) e com o modo Custom ativo.
    pub fn send_frame(&mut self, frame: &Frame) -> io::Result<()> {
        let bytes: Vec<u8> = frame.iter().flatten().copied().collect();
        self.write(cmd::DIRECT, 0, &bytes)
    }

    pub fn end_direct(&mut self) -> io::Result<()> {
        self.query(cmd::END_DIRECT, 0, &[], 0).map(|_| ())
    }
}
