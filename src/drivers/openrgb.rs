//! Teclados controlados pelo OpenRGB, pelo servidor SDK dele
//! (OpenRGB aberto com "SDK Server" ligado, porta 6742).
//!
//! O keyglow vira um cliente: pede a lista de dispositivos, põe o teclado no
//! modo direto e manda as cores. Ao sair, devolve o modo que estava ativo.
//! Protocolo: `Documentation/OpenRGBSDK.md` do OpenRGB; usamos até a versão 4.
//!
//! Se o mesmo teclado também for encontrado direto (EVision/LampArray), a
//! entrada do OpenRGB é omitida, para os dois não brigarem pelo dispositivo.

use super::{DeviceEntry, Driver, Grid, HwMode, Kind, Rgb, Source};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const DEFAULT_ADDR: ([u8; 4], u16) = ([127, 0, 0, 1], 6742);
/// Maior versão do protocolo cujo formato o keyglow conhece.
const CLIENT_PROTOCOL: u32 = 4;
const DEVICE_TYPE_KEYBOARD: i32 = 5;
const NO_LED: u32 = 0xFFFF_FFFF;
const MAX_PACKET: usize = 16 << 20;

mod pkt {
    pub const REQUEST_CONTROLLER_COUNT: u32 = 0;
    pub const REQUEST_CONTROLLER_DATA: u32 = 1;
    pub const REQUEST_PROTOCOL_VERSION: u32 = 40;
    pub const SET_CLIENT_NAME: u32 = 50;
    pub const DEVICE_LIST_UPDATED: u32 = 100;
    pub const UPDATE_LEDS: u32 = 1050;
    pub const SET_CUSTOM_MODE: u32 = 1100;
    pub const UPDATE_MODE: u32 = 1101;
}

fn addr() -> SocketAddr {
    SocketAddr::from(DEFAULT_ADDR)
}

// ---------------------------------------------------------------------------
// Conexão

struct Conn {
    stream: TcpStream,
    protocol: u32,
}

impl Conn {
    fn connect(addr: SocketAddr, timeout: Duration) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut conn = Self { stream, protocol: 0 };
        conn.send(0, pkt::REQUEST_PROTOCOL_VERSION, &CLIENT_PROTOCOL.to_le_bytes())?;
        // Servidores com protocolo 0 não respondem a este pedido.
        conn.stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let server = match conn.recv(pkt::REQUEST_PROTOCOL_VERSION) {
            Ok(data) if data.len() >= 4 => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            Ok(_) => 0,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => 0,
            Err(e) => return Err(e),
        };
        conn.stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        conn.protocol = server.min(CLIENT_PROTOCOL);
        conn.send(0, pkt::SET_CLIENT_NAME, b"keyglow\0")?;
        Ok(conn)
    }

    fn send(&mut self, dev: u32, id: u32, data: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(16 + data.len());
        buf.extend_from_slice(b"ORGB");
        buf.extend_from_slice(&dev.to_le_bytes());
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(data);
        self.stream.write_all(&buf)
    }

    fn read_packet(&mut self) -> io::Result<(u32, Vec<u8>)> {
        let mut head = [0u8; 16];
        self.stream.read_exact(&mut head)?;
        if &head[..4] != b"ORGB" {
            return Err(io::Error::other("resposta inválida do OpenRGB"));
        }
        let id = u32::from_le_bytes(head[8..12].try_into().unwrap());
        let size = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
        if size > MAX_PACKET {
            return Err(io::Error::other("pacote grande demais do OpenRGB"));
        }
        let mut data = vec![0u8; size];
        self.stream.read_exact(&mut data)?;
        Ok((id, data))
    }

    /// Espera a resposta `id`, ignorando avisos que o servidor manda sozinho.
    fn recv(&mut self, id: u32) -> io::Result<Vec<u8>> {
        loop {
            let (got, data) = self.read_packet()?;
            if got == id {
                return Ok(data);
            }
        }
    }

    fn controller_count(&mut self) -> io::Result<u32> {
        self.send(0, pkt::REQUEST_CONTROLLER_COUNT, &[])?;
        let data = self.recv(pkt::REQUEST_CONTROLLER_COUNT)?;
        let b: [u8; 4] = data.get(..4).and_then(|s| s.try_into().ok()).ok_or_else(|| io::Error::other("resposta curta"))?;
        Ok(u32::from_le_bytes(b))
    }

    fn controller(&mut self, index: u32) -> io::Result<Controller> {
        let req = if self.protocol == 0 { Vec::new() } else { self.protocol.to_le_bytes().to_vec() };
        self.send(index, pkt::REQUEST_CONTROLLER_DATA, &req)?;
        let data = self.recv(pkt::REQUEST_CONTROLLER_DATA)?;
        Controller::parse(&data, self.protocol).ok_or_else(|| io::Error::other("dados de dispositivo inválidos do OpenRGB"))
    }

    /// Lê avisos pendentes sem bloquear. Erro se a lista de dispositivos mudou
    /// (os índices deixam de valer).
    fn poll_updates(&mut self) -> io::Result<()> {
        loop {
            self.stream.set_nonblocking(true)?;
            let mut head = [0u8; 16];
            let peeked = self.stream.peek(&mut head);
            self.stream.set_nonblocking(false)?;
            match peeked {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "o OpenRGB fechou a conexão")),
                Ok(n) if n < 16 => return Ok(()),
                Ok(_) => {
                    let (id, _) = self.read_packet()?;
                    if id == pkt::DEVICE_LIST_UPDATED {
                        return Err(io::Error::other("a lista de dispositivos do OpenRGB mudou"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dados de um dispositivo

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn string(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        let s = self.take(len)?;
        Some(String::from_utf8_lossy(s).trim_end_matches('\0').to_string())
    }
}

/// (altura, largura, índice do LED na zona ou `NO_LED` para cada célula).
type Matrix = (usize, usize, Vec<u32>);

#[derive(Debug)]
struct Zone {
    leds_count: u32,
    matrix: Option<Matrix>,
}

#[derive(Debug)]
struct Controller {
    kind: i32,
    name: String,
    location: String,
    active_mode: i32,
    /// Bytes de cada modo, para devolver o modo original com UPDATE_MODE.
    modes: Vec<Vec<u8>>,
    zones: Vec<Zone>,
    leds: Vec<String>,
}

impl Controller {
    /// Interpreta a resposta de REQUEST_CONTROLLER_DATA (com o `data_size`).
    fn parse(data: &[u8], protocol: u32) -> Option<Self> {
        let mut r = Reader { b: data, i: 0 };
        r.u32()?; // data_size
        let kind = r.i32()?;
        let name = r.string()?;
        if protocol >= 1 {
            r.string()?; // vendor
        }
        r.string()?; // description
        r.string()?; // version
        r.string()?; // serial
        let location = r.string()?;

        let num_modes = r.u16()?;
        let active_mode = r.i32()?;
        let mut modes = Vec::with_capacity(num_modes as usize);
        for _ in 0..num_modes {
            let start = r.i;
            r.string()?; // nome
            // value, flags, speed_min, speed_max
            r.take(16)?;
            if protocol >= 3 {
                r.take(8)?; // brightness_min, brightness_max
            }
            r.take(12)?; // colors_min, colors_max, speed
            if protocol >= 3 {
                r.take(4)?; // brightness
            }
            r.take(8)?; // direction, color_mode
            let colors = r.u16()? as usize;
            r.take(colors * 4)?;
            modes.push(data[start..r.i].to_vec());
        }

        let num_zones = r.u16()?;
        let mut zones = Vec::with_capacity(num_zones as usize);
        for _ in 0..num_zones {
            r.string()?; // nome
            r.i32()?; // tipo
            r.take(8)?; // leds_min, leds_max
            let leds_count = r.u32()?;
            let matrix_len = r.u16()? as usize;
            let matrix = if matrix_len >= 8 {
                let h = r.u32()? as usize;
                let w = r.u32()? as usize;
                if h.checked_mul(w)?.checked_mul(4)?.checked_add(8)? != matrix_len {
                    return None;
                }
                let map = (0..h * w).map(|_| r.u32()).collect::<Option<Vec<_>>>()?;
                Some((h, w, map))
            } else {
                r.take(matrix_len)?;
                None
            };
            if protocol >= 4 {
                let segments = r.u16()?;
                for _ in 0..segments {
                    r.string()?;
                    r.take(12)?; // tipo, start_idx, leds_count
                }
            }
            zones.push(Zone { leds_count, matrix });
        }

        let num_leds = r.u16()?;
        let leds = (0..num_leds)
            .map(|_| {
                let n = r.string();
                r.u32()?; // value
                n
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { kind, name, location, active_mode, modes, zones, leds })
    }

    fn id(&self) -> String {
        format!("openrgb:{}@{}", self.name, self.location)
    }
}

/// Grade a partir da maior zona em matriz; sem matriz, uma linha com todos
/// os LEDs. Devolve também a célula de cada LED (se ele estiver na grade).
fn build_grid(c: &Controller) -> (Grid, Vec<Option<usize>>) {
    let mut led_cell = vec![None; c.leds.len()];
    let mut start = 0usize;
    let mut best: Option<(usize, &Matrix)> = None;
    for z in &c.zones {
        if let Some(m) = &z.matrix
            && best.is_none_or(|(_, b)| m.0 * m.1 > b.0 * b.1)
        {
            best = Some((start, m));
        }
        start += z.leds_count as usize;
    }
    match best {
        Some((zone_start, (h, w, map))) => {
            let mut grid = Grid { rows: *h, cols: *w, present: vec![false; h * w] };
            for (cell, &v) in map.iter().enumerate() {
                let led = zone_start + v as usize;
                if v != NO_LED && led < led_cell.len() {
                    led_cell[led] = Some(cell);
                    grid.present[cell] = true;
                }
            }
            (grid, led_cell)
        }
        None => {
            let n = c.leds.len().max(1);
            for (i, cell) in led_cell.iter_mut().enumerate() {
                *cell = Some(i);
            }
            (Grid::full(1, n), led_cell)
        }
    }
}

// ---------------------------------------------------------------------------
// Busca e driver

/// Teclados do servidor do OpenRGB. Sem servidor, lista vazia.
pub fn scan() -> Vec<DeviceEntry> {
    scan_at(addr())
}

fn scan_at(addr: SocketAddr) -> Vec<DeviceEntry> {
    let Ok(mut conn) = Conn::connect(addr, Duration::from_millis(150)) else { return Vec::new() };
    let mut out = Vec::new();
    let count = conn.controller_count().unwrap_or(0);
    for i in 0..count {
        let Ok(c) = conn.controller(i) else { break };
        if c.kind == DEVICE_TYPE_KEYBOARD && !c.leds.is_empty() {
            out.push(DeviceEntry {
                id: c.id(),
                name: c.name.clone(),
                kind: Kind::OpenRgb,
                tested: false,
                source: Source::OpenRgb { name: c.name.clone(), location: c.location.clone() },
            });
        }
    }
    out
}

/// Local do dispositivo no formato do OpenRGB, para descartar duplicatas.
pub fn hid_location(path: &str) -> String {
    format!("HID: {path}")
}

pub struct OpenRgb {
    conn: Conn,
    index: u32,
    grid: Grid,
    led_cell: Vec<Option<usize>>,
    led_names: Vec<String>,
    active_mode: i32,
    original_mode: Option<Vec<u8>>,
}

impl OpenRgb {
    pub fn open(name: &str, location: &str) -> io::Result<Self> {
        Self::open_at(addr(), name, location)
    }

    fn open_at(addr: SocketAddr, name: &str, location: &str) -> io::Result<Self> {
        let mut conn = Conn::connect(addr, Duration::from_millis(500))
            .map_err(|e| io::Error::new(e.kind(), format!("OpenRGB não está aberto com o servidor SDK ({e})")))?;
        let count = conn.controller_count()?;
        for index in 0..count {
            let c = conn.controller(index)?;
            if c.name == name && c.location == location {
                let (grid, led_cell) = build_grid(&c);
                let original_mode = usize::try_from(c.active_mode).ok().and_then(|i| c.modes.get(i).cloned());
                return Ok(Self { conn, index, grid, led_cell, led_names: c.leds, active_mode: c.active_mode, original_mode });
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, format!("o OpenRGB não lista mais {name}")))
    }
}

impl Driver for OpenRgb {
    fn grid(&self) -> &Grid {
        &self.grid
    }

    fn enter_direct(&mut self, _hw: &HwMode) -> io::Result<()> {
        self.conn.send(self.index, pkt::SET_CUSTOM_MODE, &[])
    }

    fn send_frame(&mut self, frame: &[Rgb]) -> io::Result<()> {
        self.conn.poll_updates()?;
        // LEDs fora da grade (logo, bordas) ficam com a cor média.
        let (mut sum, mut n) = ([0u32; 3], 0u32);
        for px in frame.iter().enumerate().filter(|(i, _)| self.grid.has_led(*i)).map(|(_, px)| px) {
            for k in 0..3 {
                sum[k] += px[k] as u32;
            }
            n += 1;
        }
        let avg = sum.map(|s| (s / n.max(1)) as u8);
        let count = self.led_cell.len();
        let mut data = Vec::with_capacity(6 + count * 4);
        data.extend_from_slice(&((6 + count * 4) as u32).to_le_bytes());
        data.extend_from_slice(&(count as u16).to_le_bytes());
        for cell in &self.led_cell {
            let [r, g, b] = cell.and_then(|c| frame.get(c).copied()).unwrap_or(avg);
            data.extend_from_slice(&[r, g, b, 0]);
        }
        self.conn.send(self.index, pkt::UPDATE_LEDS, &data)
    }

    fn leave_direct(&mut self, _hw: &HwMode) -> io::Result<()> {
        let Some(mode) = &self.original_mode else { return Ok(()) };
        let mut data = Vec::with_capacity(8 + mode.len());
        data.extend_from_slice(&((8 + mode.len()) as u32).to_le_bytes());
        data.extend_from_slice(&self.active_mode.to_le_bytes());
        data.extend_from_slice(mode);
        self.conn.send(self.index, pkt::UPDATE_MODE, &data)
    }

    fn default_key_map(&self) -> BTreeMap<u16, usize> {
        self.led_names
            .iter()
            .zip(&self.led_cell)
            .filter_map(|(name, cell)| Some((crate::keymap::openrgb_name_to_evdev(name)?, (*cell)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn put_str(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&((s.len() + 1) as u16).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }

    /// Bloco de modo no protocolo 4.
    fn mode(name: &str, value: i32) -> Vec<u8> {
        let mut b = Vec::new();
        put_str(&mut b, name);
        b.extend_from_slice(&value.to_le_bytes());
        for v in [0u32, 0, 4, 0, 100, 0, 0, 2, 100, 0, 1] {
            // flags, speed_min, speed_max, brightness_min, brightness_max,
            // colors_min, colors_max, speed, brightness, direction, color_mode
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0x00FF00u32.to_le_bytes());
        b
    }

    /// Teclado de 2x2 numa zona em matriz (A, S / Z, vazio) + zona "Logo".
    fn keyboard(name: &str, location: &str) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&DEVICE_TYPE_KEYBOARD.to_le_bytes());
        for s in [name, "Vendor", "desc", "1.0", "serial", location] {
            put_str(&mut d, s);
        }
        d.extend_from_slice(&2u16.to_le_bytes()); // modos
        d.extend_from_slice(&1i32.to_le_bytes()); // ativo: "Static"
        d.extend(mode("Direct", 0));
        d.extend(mode("Static", 1));
        d.extend_from_slice(&2u16.to_le_bytes()); // zonas
        put_str(&mut d, "Keyboard");
        d.extend_from_slice(&2i32.to_le_bytes());
        for v in [3u32, 3, 3] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&((2 * 2 * 4 + 8) as u16).to_le_bytes());
        for v in [2u32, 2, 0, 1, 2, NO_LED] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes()); // segmentos
        put_str(&mut d, "Logo");
        d.extend_from_slice(&0i32.to_le_bytes());
        for v in [1u32, 1, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes()); // sem matriz
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&4u16.to_le_bytes()); // LEDs
        for n in ["Key: A", "Key: S", "Key: Z", "Logo"] {
            put_str(&mut d, n);
            d.extend_from_slice(&0u32.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes()); // cores
        let mut out = ((d.len() + 4) as u32).to_le_bytes().to_vec();
        out.extend(d);
        out
    }

    /// Servidor OpenRGB de mentira: um teclado e um mouse; guarda os
    /// pacotes de controle recebidos.
    /// Pacotes recebidos pelo servidor: (dispositivo, id, dados).
    type Received = Arc<Mutex<Vec<(u32, u32, Vec<u8>)>>>;

    fn fake_server() -> (SocketAddr, Received) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let got2 = Arc::clone(&got);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut s = stream.unwrap();
                let got = Arc::clone(&got2);
                thread::spawn(move || {
                    let reply = |s: &mut TcpStream, dev: u32, id: u32, data: &[u8]| {
                        let mut b = b"ORGB".to_vec();
                        for v in [dev, id, data.len() as u32] {
                            b.extend_from_slice(&v.to_le_bytes());
                        }
                        b.extend_from_slice(data);
                        s.write_all(&b).unwrap();
                    };
                    loop {
                        let mut head = [0u8; 16];
                        if s.read_exact(&mut head).is_err() {
                            return;
                        }
                        let dev = u32::from_le_bytes(head[4..8].try_into().unwrap());
                        let id = u32::from_le_bytes(head[8..12].try_into().unwrap());
                        let mut data = vec![0u8; u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize];
                        s.read_exact(&mut data).unwrap();
                        match id {
                            pkt::REQUEST_PROTOCOL_VERSION => {
                                // Avisos soltos no meio não podem atrapalhar.
                                reply(&mut s, 0, 103, &[]);
                                reply(&mut s, 0, id, &5u32.to_le_bytes());
                            }
                            pkt::REQUEST_CONTROLLER_COUNT => reply(&mut s, 0, id, &2u32.to_le_bytes()),
                            pkt::REQUEST_CONTROLLER_DATA => {
                                assert_eq!(data, 4u32.to_le_bytes(), "cliente deve pedir o protocolo 4");
                                if dev == 0 {
                                    let mut mouse = keyboard("Mouse", "HID: m");
                                    mouse[4..8].copy_from_slice(&6i32.to_le_bytes());
                                    reply(&mut s, dev, id, &mouse);
                                } else {
                                    reply(&mut s, dev, id, &keyboard("Teclado X", "HID: /dev/hidraw9"));
                                }
                            }
                            pkt::SET_CLIENT_NAME => {}
                            _ => got.lock().unwrap().push((dev, id, data)),
                        }
                    }
                });
            }
        });
        (addr, got)
    }

    #[test]
    fn scan_lists_only_keyboards() {
        let (addr, _) = fake_server();
        let found = scan_at(addr);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Teclado X");
        assert_eq!(found[0].id, "openrgb:Teclado X@HID: /dev/hidraw9");
    }

    #[test]
    fn scan_without_server_is_empty() {
        // Porta sem ninguém escutando.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        assert!(scan_at(addr).is_empty());
    }

    #[test]
    fn direct_mode_round_trip() {
        let (addr, got) = fake_server();
        let mut kb = OpenRgb::open_at(addr, "Teclado X", "HID: /dev/hidraw9").unwrap();
        assert_eq!((kb.grid.rows, kb.grid.cols), (2, 2));
        assert_eq!(kb.grid.present, vec![true, true, true, false]);
        let keys = kb.default_key_map();
        assert_eq!((keys[&30], keys[&31], keys[&44]), (0, 1, 2));

        kb.enter_direct(&HwMode::default()).unwrap();
        kb.send_frame(&[[10, 0, 0], [0, 20, 0], [0, 0, 30], [99, 99, 99]]).unwrap();
        kb.leave_direct(&HwMode::default()).unwrap();
        thread::sleep(Duration::from_millis(100));

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].0, got[0].1), (1, pkt::SET_CUSTOM_MODE));
        let leds = &got[1].2;
        assert_eq!(got[1].1, pkt::UPDATE_LEDS);
        assert_eq!(u32::from_le_bytes(leds[0..4].try_into().unwrap()) as usize, leds.len());
        assert_eq!(u16::from_le_bytes([leds[4], leds[5]]), 4);
        assert_eq!(&leds[6..22], &[10, 0, 0, 0, 0, 20, 0, 0, 0, 0, 30, 0, 3, 6, 10, 0]);
        // Volta ao modo "Static" (índice 1) com os bytes originais.
        let m = &got[2].2;
        assert_eq!(got[2].1, pkt::UPDATE_MODE);
        assert_eq!(i32::from_le_bytes(m[4..8].try_into().unwrap()), 1);
        assert_eq!(&m[8..], mode("Static", 1).as_slice());
    }
}
