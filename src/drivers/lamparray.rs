//! Padrão HID LampArray (usage page 0x59), o mesmo da Iluminação Dinâmica
//! do Windows 11. Um driver para qualquer fabricante que siga o padrão.
//!
//! O dispositivo descreve cada LED ("lamp"): posição em micrômetros e, nos
//! teclados, qual tecla fica sobre ele. Com isso o keyglow monta a grade e o
//! mapa de teclas sem calibração.
//!
//! Os report IDs vêm do descriptor (como faz o OpenRGB) e o tamanho de cada
//! relatório é conferido com o layout do exemplo da Microsoft. Se for
//! diferente, o dispositivo é recusado em vez de receber bytes no lugar errado.
//!
//! No Windows, a Iluminação Dinâmica também controla esses dispositivos;
//! desligue-a para o dispositivo (Configurações > Personalização >
//! Iluminação dinâmica) se as cores brigarem.

use super::{DeviceEntry, Driver, Grid, HwMode, Kind, Rgb, Source};
use crate::hid::descriptor::{feature_report_sizes, report_ids_by_usage};
use crate::hid::{HidInfo, HidIo, Port};
use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

pub const LIGHTING_PAGE: u16 = 0x59;
const USAGE_LAMP_ARRAY: u16 = 0x01;

mod usage {
    pub const ATTRIBUTES: u16 = 0x02;
    pub const ATTRIBUTES_REQUEST: u16 = 0x20;
    pub const ATTRIBUTES_RESPONSE: u16 = 0x22;
    pub const MULTI_UPDATE: u16 = 0x50;
    pub const CONTROL: u16 = 0x70;
}

/// Tamanho (sem o report ID) de cada relatório no layout da Microsoft.
const ATTRIBUTES_LEN: usize = 22;
const REQUEST_LEN: usize = 2;
const RESPONSE_LEN: usize = 28;
const MULTI_LAMPS: usize = 8;
const MULTI_LEN: usize = 2 + MULTI_LAMPS * 2 + MULTI_LAMPS * 4;
const UPDATE_COMPLETE: u8 = 1;
const MAX_LAMPS: u16 = 1024;

/// Distância entre teclas vizinhas (1u = 19,05 mm), em micrômetros.
const KEY_PITCH_UM: f32 = 19_050.0;
/// LEDs com Y até esta distância do primeiro da linha ficam na mesma linha.
const ROW_TOLERANCE_UM: u32 = 7_000;
const MAX_COLS: usize = 32;

pub fn recognize(info: &HidInfo) -> Option<DeviceEntry> {
    if info.usage_page != LIGHTING_PAGE || info.usage != USAGE_LAMP_ARRAY {
        return None;
    }
    let name = if info.product.is_empty() { format!("LampArray {:04x}:{:04x}", info.vid, info.pid) } else { info.product.clone() };
    Some(DeviceEntry {
        id: format!("lamparray:{:04x}:{:04x}", info.vid, info.pid),
        name,
        kind: Kind::LampArray,
        tested: false,
        source: Source::Hid(info.clone()),
    })
}

struct ReportIds {
    attributes: u8,
    request: u8,
    response: u8,
    multi: u8,
    control: u8,
}

#[derive(Clone, Copy, Debug)]
struct Lamp {
    id: u16,
    x: u32,
    y: u32,
    /// Código HID (página Keyboard) da tecla sobre o LED; 0 = nenhuma.
    key: u8,
}

pub struct LampArray {
    port: Box<dyn HidIo>,
    ids: ReportIds,
    lamps: Vec<Lamp>,
    /// Célula da grade de cada lamp.
    cell: Vec<usize>,
    grid: Grid,
    /// Última cor enviada a cada lamp (só mandamos o que mudou).
    last: Vec<Option<[u8; 4]>>,
    min_interval: Duration,
    last_send: Option<Instant>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl LampArray {
    pub fn open(info: &HidInfo) -> io::Result<Self> {
        Self::with_io(Box::new(Port::open(info)?))
    }

    pub fn with_io(mut port: Box<dyn HidIo>) -> io::Result<Self> {
        let desc = port.report_descriptor()?;
        let by_usage = report_ids_by_usage(&desc, LIGHTING_PAGE);
        let sizes = feature_report_sizes(&desc);
        let unsupported = |what: &str| io::Error::other(format!("LampArray fora do padrão ({what}); dispositivo não suportado"));
        let id_of = |u: u16, len: usize, exact: bool| -> io::Result<u8> {
            let id = *by_usage.get(&u).ok_or_else(|| unsupported(&format!("falta o relatório 0x{u:02x}")))?;
            let size = sizes.get(&id).copied().unwrap_or(0);
            if size < len || (exact && size != len) {
                return Err(unsupported(&format!("relatório 0x{u:02x} com {size} bytes")));
            }
            Ok(id)
        };
        let ids = ReportIds {
            attributes: id_of(usage::ATTRIBUTES, ATTRIBUTES_LEN, false)?,
            request: id_of(usage::ATTRIBUTES_REQUEST, REQUEST_LEN, false)?,
            response: id_of(usage::ATTRIBUTES_RESPONSE, RESPONSE_LEN, false)?,
            multi: id_of(usage::MULTI_UPDATE, MULTI_LEN, true)?,
            control: id_of(usage::CONTROL, 1, false)?,
        };

        // Atributos gerais: quantidade de lamps e intervalo mínimo.
        let mut buf = vec![0u8; 1 + sizes[&ids.attributes]];
        buf[0] = ids.attributes;
        port.get_feature(&mut buf)?;
        let count = u16_at(&buf, 1);
        let min_interval = Duration::from_millis(u32_at(&buf, 1 + 18).min(1000) as u64);
        if count == 0 || count > MAX_LAMPS {
            return Err(unsupported(&format!("{count} LEDs")));
        }

        // Pede os atributos a partir do lamp 0; o dispositivo avança sozinho.
        let mut req = vec![0u8; 1 + sizes[&ids.request]];
        req[0] = ids.request;
        port.send_feature(&req)?;
        let mut lamps = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let mut buf = vec![0u8; 1 + sizes[&ids.response]];
            buf[0] = ids.response;
            port.get_feature(&mut buf)?;
            lamps.push(Lamp { id: u16_at(&buf, 1), x: u32_at(&buf, 3), y: u32_at(&buf, 7), key: buf[1 + 27] });
        }

        let positions: Vec<(u32, u32)> = lamps.iter().map(|l| (l.x, l.y)).collect();
        let (grid, cell) = build_grid(&positions);
        let n = lamps.len();
        Ok(Self { port, ids, lamps, cell, grid, last: vec![None; n], min_interval, last_send: None })
    }

    fn set_autonomous(&mut self, on: bool) -> io::Result<()> {
        self.port.send_feature(&[self.ids.control, on as u8])
    }
}

/// Monta a grade a partir das posições (x, y) dos LEDs: linhas pelo Y
/// (agrupando os próximos) e colunas pelo X, em unidades de tecla.
fn build_grid(pos: &[(u32, u32)]) -> (Grid, Vec<usize>) {
    if pos.is_empty() {
        return (Grid::full(1, 1), Vec::new());
    }
    let mut ys: Vec<u32> = pos.iter().map(|p| p.1).collect();
    ys.sort_unstable();
    ys.dedup();
    // Início (menor Y) de cada linha.
    let mut row_starts = vec![ys[0]];
    for &y in &ys[1..] {
        if y - row_starts.last().unwrap() > ROW_TOLERANCE_UM {
            row_starts.push(y);
        }
    }
    let row_of = |y: u32| row_starts.iter().rposition(|&s| s <= y).unwrap_or(0);

    let min_x = pos.iter().map(|p| p.0).min().unwrap();
    let max_x = pos.iter().map(|p| p.0).max().unwrap();
    let pitch = KEY_PITCH_UM.max((max_x - min_x) as f32 / (MAX_COLS - 1) as f32);
    let col_of = |x: u32| ((x - min_x) as f32 / pitch).round() as usize;
    let cols = col_of(max_x) + 1;

    let rows = row_starts.len();
    let mut grid = Grid { rows, cols, present: vec![false; rows * cols] };
    let cell: Vec<usize> = pos.iter().map(|&(x, y)| row_of(y) * cols + col_of(x)).collect();
    for &c in &cell {
        grid.present[c] = true;
    }
    (grid, cell)
}

impl Driver for LampArray {
    fn grid(&self) -> &Grid {
        &self.grid
    }

    fn enter_direct(&mut self, _hw: &HwMode) -> io::Result<()> {
        self.last.fill(None);
        self.set_autonomous(false)
    }

    fn send_frame(&mut self, frame: &[Rgb]) -> io::Result<()> {
        if self.last_send.is_some_and(|t| t.elapsed() < self.min_interval) {
            return Ok(());
        }
        let changed: Vec<(usize, [u8; 4])> = self
            .cell
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let [r, g, b] = frame.get(c).copied().unwrap_or([0; 3]);
                (i, [r, g, b, 255])
            })
            .filter(|&(i, color)| self.last[i] != Some(color))
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        let chunks = changed.chunks(MULTI_LAMPS).count();
        for (n, chunk) in changed.chunks(MULTI_LAMPS).enumerate() {
            let mut buf = [0u8; 1 + MULTI_LEN];
            buf[0] = self.ids.multi;
            buf[1] = chunk.len() as u8;
            buf[2] = if n + 1 == chunks { UPDATE_COMPLETE } else { 0 };
            for (k, &(i, color)) in chunk.iter().enumerate() {
                buf[3 + k * 2..5 + k * 2].copy_from_slice(&self.lamps[i].id.to_le_bytes());
                let at = 3 + MULTI_LAMPS * 2 + k * 4;
                buf[at..at + 4].copy_from_slice(&color);
            }
            self.port.send_feature(&buf)?;
            for &(i, color) in chunk {
                self.last[i] = Some(color);
            }
        }
        self.last_send = Some(Instant::now());
        Ok(())
    }

    fn leave_direct(&mut self, _hw: &HwMode) -> io::Result<()> {
        self.set_autonomous(true)
    }

    /// Uma imagem pulada pelo intervalo mínimo precisa ser reenviada depois;
    /// reenviar a mesma imagem não custa nada (só vai o que mudou).
    fn keepalive(&self) -> Option<Duration> {
        Some(self.min_interval.max(Duration::from_millis(100)))
    }

    fn default_key_map(&self) -> BTreeMap<u16, usize> {
        self.lamps
            .iter()
            .zip(&self.cell)
            .filter_map(|(l, &c)| crate::keymap::hid_usage_to_evdev(l.key as u16).map(|code| (code, c)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::descriptor::tests::LAMPARRAY_SAMPLE;
    use std::sync::{Arc, Mutex};

    /// LampArray de mentira com o descriptor da Microsoft e 3 LEDs:
    /// Esc e F1 na primeira linha, A na segunda.
    struct MockLamps {
        next: u16,
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    const LAMPS: [(u32, u32, u8); 3] = [(5_000, 5_000, 0x29), (43_100, 6_000, 0x3A), (10_000, 24_000, 0x04)];

    impl HidIo for MockLamps {
        fn write(&mut self, _buf: &[u8]) -> io::Result<()> {
            panic!("LampArray usa só relatórios de feature")
        }
        fn read_timeout(&mut self, _buf: &mut [u8], _t: i32) -> io::Result<usize> {
            Ok(0)
        }
        fn report_descriptor(&mut self) -> io::Result<Vec<u8>> {
            Ok(LAMPARRAY_SAMPLE.to_vec())
        }
        fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match buf[0] {
                1 => {
                    buf[1..3].copy_from_slice(&(LAMPS.len() as u16).to_le_bytes());
                    buf[15..19].copy_from_slice(&1u32.to_le_bytes()); // teclado
                }
                3 => {
                    let (x, y, key) = LAMPS[self.next as usize];
                    buf[1..3].copy_from_slice(&self.next.to_le_bytes());
                    buf[3..7].copy_from_slice(&x.to_le_bytes());
                    buf[7..11].copy_from_slice(&y.to_le_bytes());
                    buf[28] = key;
                    self.next += 1;
                }
                id => panic!("relatório {id} inesperado"),
            }
            Ok(buf.len())
        }
        fn send_feature(&mut self, buf: &[u8]) -> io::Result<()> {
            if buf[0] == 2 {
                self.next = u16_at(buf, 1);
            }
            self.sent.lock().unwrap().push(buf.to_vec());
            Ok(())
        }
    }

    fn open() -> (LampArray, Arc<Mutex<Vec<Vec<u8>>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let la = LampArray::with_io(Box::new(MockLamps { next: 0, sent: Arc::clone(&sent) })).unwrap();
        sent.lock().unwrap().clear();
        (la, sent)
    }

    #[test]
    fn grid_and_keys_from_lamps() {
        let (la, _) = open();
        // Esc em (0,0), F1 duas teclas à direita, A na linha de baixo.
        assert_eq!((la.grid.rows, la.grid.cols), (2, 3));
        let keys = la.default_key_map();
        assert_eq!(keys[&1], 0); // Esc
        assert_eq!(keys[&59], 2); // F1
        assert_eq!(keys[&30], 3); // A
        assert_eq!(la.grid.present, vec![true, false, true, true, false, false]);
    }

    #[test]
    fn sends_only_changes_and_restores_autonomous() {
        let (mut la, sent) = open();
        la.enter_direct(&HwMode::default()).unwrap();
        let mut frame = vec![[10, 20, 30]; la.grid.len()];
        la.send_frame(&frame).unwrap();
        frame[3] = [1, 2, 3];
        la.send_frame(&frame).unwrap();
        la.send_frame(&frame).unwrap(); // nada mudou: não envia
        la.leave_direct(&HwMode::default()).unwrap();

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0], vec![6, 0]); // autônomo desligado
        let first = &sent[1];
        assert_eq!((first[0], first[1], first[2]), (4, 3, UPDATE_COMPLETE));
        assert_eq!(&first[19..23], &[10, 20, 30, 255]);
        let second = &sent[2];
        assert_eq!((second[1], u16_at(second, 3)), (1, 2)); // só o lamp 2 (A)
        assert_eq!(&second[19..23], &[1, 2, 3, 255]);
        assert_eq!(sent[3], vec![6, 1]); // autônomo de volta
    }

    /// Imagem que chega antes do intervalo mínimo não é perdida: vai na
    /// próxima chamada depois do intervalo.
    #[test]
    fn throttled_frame_is_sent_later() {
        let (mut la, sent) = open();
        la.min_interval = Duration::from_millis(40);
        la.enter_direct(&HwMode::default()).unwrap();
        la.send_frame(&vec![[1, 1, 1]; la.grid.len()]).unwrap();
        let b = vec![[2, 2, 2]; la.grid.len()];
        la.send_frame(&b).unwrap(); // cedo demais: pulada
        assert_eq!(sent.lock().unwrap().len(), 2);
        std::thread::sleep(Duration::from_millis(50));
        la.send_frame(&b).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        assert_eq!(&sent[2][19..23], &[2, 2, 2, 255]);
        assert!(la.keepalive().is_some());
    }

    #[test]
    fn grid_caps_wide_layouts() {
        let pos: Vec<(u32, u32)> = (0..100).map(|i| (i * 19_050, 0)).collect();
        let (grid, cell) = build_grid(&pos);
        assert!(grid.cols <= MAX_COLS);
        assert!(cell.iter().all(|&c| c < grid.len()));
    }
}
