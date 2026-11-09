//! Teclados SONiX/EVision com firmware "EVision V2" (o mesmo protocolo que o
//! OpenRGB documenta), falando pela interface de controle 0xFF1C.
//!
//! Formato de cada pacote (64 bytes):
//! `[0x04, chk_lo, chk_hi, cmd, tamanho, off_lo, off_hi, status, dados...]`
//! onde o checksum é a soma (u16) dos bytes 3..64.
//!
//! Modelos:
//! - `320f:5016` (Husky Anchorage HTG200): testado. Os números dos modos de fábrica são os do OpenRGB
//!   menos 1 (5 = fixo, 0x13 = custom).
//! - `320f:5041` e `3299:0012` (Endorfy Omnis): suportados pelo OpenRGB,
//!   não testados aqui; usam a numeração do OpenRGB.
//! - `0c45:8501` (Husky Sled HTG500): mesmo protocolo, numeração do OpenRGB; não testado.
//! - Outros `320f`/`3299` com a interface 0xFF1C: não testados. Ao abrir,
//!   o keyglow lê as capacidades (comando só de leitura) e só segue se o
//!   firmware responder como EVision V2.
//! - Os modelos "EVision V1" do OpenRGB usam outro protocolo e são ignorados.
//!
//! Além da luz, alguns modelos guardam na memória o que cada tecla faz e
//! macros (formatos em [`crate::onboard`]). Isso só é liberado nos modelos
//! da tabela marcados com `onboard`.

use super::{DeviceEntry, Driver, Grid, HwMode, Kind, Onboard, Rgb, Source};
use crate::hid::{HidInfo, HidIo, Port};
use crate::onboard::{self, Cell};
use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

/// Usage page da interface de controle do firmware EVision.
pub const CONTROL_USAGE_PAGE: u16 = 0xFF1C;
const EVISION_VIDS: &[u16] = &[0x320F, 0x3299];

/// Teclados EVision V1 (outro protocolo; lista do OpenRGB).
const V1_MODELS: &[(u16, u16)] = &[(0x320F, 0x5000), (0x320F, 0x502A), (0x320F, 0x505B), (0x320F, 0x5064), (0x320F, 0x5078), (0x320F, 0x5084)];

struct Model {
    vid: u16,
    pid: u16,
    name: &'static str,
    /// Valor no firmware = número do modo no keyglow + `mode_offset`.
    mode_offset: u8,
    tested: bool,
    /// Mapa de teclas e macros na memória do teclado (matriz 6 x 21).
    onboard: bool,
}

const MODELS: &[Model] = &[
    Model { vid: 0x320F, pid: 0x5016, name: "Husky Anchorage HTG200", mode_offset: 0, tested: true, onboard: true },
    Model { vid: 0x320F, pid: 0x5041, name: "Teclado EVision 320f:5041", mode_offset: 1, tested: false, onboard: false },
    Model { vid: 0x3299, pid: 0x0012, name: "Endorfy Omnis", mode_offset: 1, tested: false, onboard: false },
    // Mesmo protocolo com outro ID USB. A numeração dos modos começa em 1
    // (0 = luz apagada); se o modo direto usa 0x14 nele ainda não foi visto
    // num teclado de verdade.
    Model { vid: 0x0C45, pid: 0x8501, name: "Husky Sled HTG500", mode_offset: 1, tested: false, onboard: true },
];

fn find_model(vid: u16, pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.vid == vid && m.pid == pid)
}

/// Modelos desconhecidos seguem a numeração do OpenRGB.
const UNKNOWN_MODE_OFFSET: u8 = 1;

const REPORT_ID: u8 = 0x04;
const PACKET: usize = 64;
const MAX_PAYLOAD: usize = PACKET - 8;
const TIMEOUT_MS: i32 = 500;
const ROWS: usize = 6;
const COLS: usize = 21;

mod cmd {
    pub const BEGIN: u8 = 0x01;
    pub const END: u8 = 0x02;
    pub const READ_CAPABILITIES: u8 = 0x03;
    pub const READ_CONFIG: u8 = 0x05;
    pub const WRITE_CONFIG: u8 = 0x06;
    /// Cores enviadas em tempo real pelo software.
    pub const DIRECT: u8 = 0x12;
    pub const END_DIRECT: u8 = 0x13;
    /// Tabela de teclas (378 bytes, coluna por coluna).
    pub const READ_KEYS: u8 = 0x08;
    pub const WRITE_KEYS: u8 = 0x09;
    /// Volta luz, teclas e macros ao padrão de fábrica.
    pub const FACTORY_RESET: u8 = 0x0D;
    /// Bloco de macros.
    pub const READ_MACROS: u8 = 0x14;
    pub const WRITE_MACROS: u8 = 0x15;
}

const OFF_CURRENT_PROFILE: u16 = 0x00;
const OFF_FIRST_PROFILE: u16 = 0x01;
const PROFILE_STRIDE: u16 = 0x40;

/// Modo em que o firmware não desenha efeito nenhum (no 320f:5016, o valor
/// que apaga a luz). Com ele ativo, o modo direto fica estável, porque só as
/// cores mandadas pelo app aparecem.
pub const MODE_CUSTOM: u8 = 0x13;
pub const MODE_STATIC: u8 = 5;
/// O firmware sai do modo direto se ficar ~0,5 s sem imagens.
const KEEPALIVE: Duration = Duration::from_millis(150);

/// Efeitos de fábrica (número no keyglow, nome). Confirmados na prática no
/// 320f:5016: 5, 6 e 7.
pub const HW_MODES: &[(u8, &str)] = &[
    (0, "Onda curta"),
    (1, "Onda longa"),
    (2, "Difusão"),
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
    (16, "Apagar ao digitar"),
    (17, "Círculo arco-íris"),
];

pub fn hw_mode_name(mode: u8) -> String {
    HW_MODES
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("Modo {mode}"))
}

/// Reconhece a interface de controle de um teclado EVision V2 só pelos
/// dados da enumeração (não abre nada).
pub fn recognize(info: &HidInfo) -> Option<DeviceEntry> {
    let model = find_model(info.vid, info.pid);
    let known_vendor = EVISION_VIDS.contains(&info.vid) || model.is_some();
    if info.usage_page != CONTROL_USAGE_PAGE || !known_vendor || V1_MODELS.contains(&(info.vid, info.pid)) {
        return None;
    }
    let name = match model {
        Some(m) => m.name.to_string(),
        None if !info.product.is_empty() => format!("{} ({:04x}:{:04x})", info.product, info.vid, info.pid),
        None => format!("Teclado EVision {:04x}:{:04x}", info.vid, info.pid),
    };
    Some(DeviceEntry {
        id: format!("evision:{:04x}:{:04x}", info.vid, info.pid),
        name,
        kind: Kind::Evision,
        tested: model.is_some_and(|m| m.tested),
        source: Source::Hid(info.clone()),
    })
}

pub struct Evision {
    port: Box<dyn HidIo>,
    mode_offset: u8,
    /// Quantas posições o firmware aceita no modo direto.
    map_size: usize,
    grid: Grid,
    is_5016: bool,
    /// O modelo guarda teclas e macros no formato que o keyglow conhece.
    onboard: bool,
    firmware: Option<u16>,
}

impl Evision {
    pub fn open(info: &HidInfo) -> io::Result<Self> {
        let model = find_model(info.vid, info.pid);
        let port = Box::new(Port::open(info)?);
        let mut kb = Self::with_io(port, model.map_or(UNKNOWN_MODE_OFFSET, |m| m.mode_offset));
        kb.is_5016 = (info.vid, info.pid) == (0x320F, 0x5016);
        kb.onboard = model.is_some_and(|m| m.onboard);
        kb.firmware = (info.release != 0).then_some(info.release);
        if !kb.is_5016 {
            kb.check_capabilities()?;
        }
        Ok(kb)
    }

    pub fn with_io(port: Box<dyn HidIo>, mode_offset: u8) -> Self {
        Self {
            port,
            mode_offset,
            map_size: ROWS * COLS,
            grid: Grid::full(ROWS, COLS),
            is_5016: false,
            onboard: false,
            firmware: None,
        }
    }

    /// Confirma que o firmware fala EVision V2 (resposta `AA 55`) e lê
    /// quantas posições de LED ele tem. Só leitura.
    fn check_capabilities(&mut self) -> io::Result<()> {
        let caps = self.read(cmd::READ_CAPABILITIES, 0, 7)?;
        if caps[0] != 0xAA || caps[1] != 0x55 {
            return Err(io::Error::other("o firmware não respondeu como EVision V2; teclado não suportado"));
        }
        if (1..ROWS * COLS).contains(&(caps[5] as usize)) {
            self.map_size = caps[5] as usize;
        }
        Ok(())
    }

    fn packet(command: u8, offset: u16, data: &[u8], size: u8) -> [u8; PACKET] {
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
        buf
    }

    /// Envia um pacote e espera a resposta correspondente.
    fn query(&mut self, command: u8, offset: u16, data: &[u8], size: u8) -> io::Result<Vec<u8>> {
        let buf = Self::packet(command, offset, data, size);
        self.port.write(&buf)?;

        // A mesma interface também manda eventos do botão de volume
        // (outros report IDs), então ignoramos o que não for resposta.
        let mut resp = [0u8; PACKET];
        loop {
            let n = self.port.read_timeout(&mut resp, TIMEOUT_MS)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "o teclado não respondeu"));
            }
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
            mode: d[0].wrapping_sub(self.mode_offset),
            brightness: d[1],
            speed: d[2],
            direction: d[3],
            random: d[4] != 0,
            color: [d[5], d[6], d[7]],
        })
    }

    fn write_hw_mode(&mut self, m: &HwMode) -> io::Result<()> {
        let profile = self.current_profile()?;
        let data = [
            m.mode.wrapping_add(self.mode_offset),
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

    fn end_direct(&mut self) -> io::Result<()> {
        self.query(cmd::END_DIRECT, 0, &[], 0).map(|_| ())
    }

    #[cfg(test)]
    pub fn enable_onboard_for_tests(&mut self) {
        self.onboard = true;
    }

    /// Grava entre os comandos de início e fim, como nos efeitos de fábrica.
    fn write_wrapped(&mut self, command: u8, data: &[u8]) -> io::Result<()> {
        self.query(cmd::BEGIN, 0, &[], 0)?;
        let result = self.write(command, 0, data);
        self.query(cmd::END, 0, &[], 0)?;
        result
    }
}

impl Onboard for Evision {
    fn read_keys(&mut self) -> io::Result<Vec<Cell>> {
        let raw = self.read(cmd::READ_KEYS, 0, onboard::TABLE_LEN)?;
        onboard::table_from_firmware(&raw).ok_or_else(|| io::Error::other("tabela de teclas incompleta"))
    }

    fn write_keys(&mut self, cells: &[Cell]) -> io::Result<()> {
        if cells.len() != onboard::SLOTS {
            return Err(io::Error::other("tabela de teclas com tamanho errado"));
        }
        self.write_wrapped(cmd::WRITE_KEYS, &onboard::table_to_firmware(cells))
    }

    fn read_macros(&mut self) -> io::Result<Vec<u8>> {
        let mut block = self.read(cmd::READ_MACROS, 0, MAX_PAYLOAD)?;
        if let Some(len) = onboard::macro_block_len(&block)
            && len > block.len()
        {
            let rest = self.read(cmd::READ_MACROS, block.len() as u16, len - block.len())?;
            block.extend_from_slice(&rest);
        }
        Ok(block)
    }

    fn write_macros(&mut self, block: &[u8]) -> io::Result<()> {
        if onboard::macro_block_len(block) != Some(block.len()) {
            return Err(io::Error::other("bloco de macros inválido"));
        }
        self.write_wrapped(cmd::WRITE_MACROS, block)
    }

    /// O teclado pode reiniciar sem responder; basta o pedido ter saído.
    fn factory_reset(&mut self) -> io::Result<()> {
        self.port.write(&Self::packet(cmd::FACTORY_RESET, 0, &[], 0))?;
        let mut resp = [0u8; PACKET];
        let _ = self.port.read_timeout(&mut resp, TIMEOUT_MS);
        Ok(())
    }
}

impl Driver for Evision {
    fn grid(&self) -> &Grid {
        &self.grid
    }

    /// O modo direto só fica estável com o firmware no modo Custom.
    fn enter_direct(&mut self, hw: &HwMode) -> io::Result<()> {
        self.set_hw_mode(&HwMode { mode: MODE_CUSTOM, brightness: 4, ..*hw })
    }

    /// Envia uma imagem completa. O teclado só mantém a imagem enquanto
    /// novas chegam (veja [`KEEPALIVE`]) e com o modo Custom ativo.
    fn send_frame(&mut self, frame: &[Rgb]) -> io::Result<()> {
        let bytes: Vec<u8> = frame.iter().take(self.map_size).flatten().copied().collect();
        self.write(cmd::DIRECT, 0, &bytes)
    }

    fn leave_direct(&mut self, hw: &HwMode) -> io::Result<()> {
        let ended = self.end_direct();
        let restored = self.set_hw_mode(hw);
        ended.and(restored)
    }

    fn connect(&mut self, saved: &HwMode) -> io::Result<Option<HwMode>> {
        let current = self.read_hw_mode()?;
        // Se o app fechou no meio de um efeito de software, o teclado fica
        // no modo Custom (apagado). Restauramos o efeito salvo.
        if current.mode == MODE_CUSTOM {
            let _ = self.set_hw_mode(saved);
            return Ok(Some(*saved));
        }
        Ok(Some(current))
    }

    fn hw_modes(&self) -> &'static [(u8, &'static str)] {
        HW_MODES
    }

    /// Grava um efeito de fábrica no perfil ativo.
    fn set_hw_mode(&mut self, m: &HwMode) -> io::Result<()> {
        self.query(cmd::BEGIN, 0, &[], 0)?;
        let result = self.write_hw_mode(m);
        self.query(cmd::END, 0, &[], 0)?;
        result
    }

    fn keepalive(&self) -> Option<Duration> {
        Some(KEEPALIVE)
    }

    fn default_key_map(&self) -> BTreeMap<u16, usize> {
        if self.is_5016 { crate::default_layout::default_key_to_led() } else { BTreeMap::new() }
    }

    fn onboard(&mut self) -> Option<&mut dyn Onboard> {
        (self.onboard && self.map_size == onboard::SLOTS).then_some(self as &mut dyn Onboard)
    }

    fn firmware_version(&self) -> Option<u16> {
        self.firmware
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::mock::{check_golden, MockEvision};
    use std::sync::{Arc, Mutex};

    /// Sequência que o motor faz com o 320f:5016: lê o efeito, grava outro,
    /// entra no modo direto, manda imagens e volta ao efeito de fábrica.
    /// Os bytes têm que ser idênticos aos do keyglow antes dos drivers.
    #[test]
    fn golden_320f_5016() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut kb = Evision::with_io(Box::new(MockEvision::new(Arc::clone(&writes))), 0);

        let current = kb.read_hw_mode().unwrap();
        assert_eq!(current, HwMode { mode: 5, brightness: 4, speed: 3, direction: 0, random: false, color: [255, 0, 0] });

        let hw = HwMode { mode: 7, brightness: 3, speed: 2, direction: 1, random: true, color: [1, 2, 3] };
        kb.set_hw_mode(&hw).unwrap();
        assert_eq!(kb.read_hw_mode().unwrap(), hw);

        kb.enter_direct(&hw).unwrap();
        let mut frame = vec![[0u8; 3]; ROWS * COLS];
        for (i, px) in frame.iter_mut().enumerate() {
            *px = [i as u8, (i * 2) as u8, 255 - i as u8];
        }
        kb.send_frame(&frame).unwrap();
        frame[5] = [9, 9, 9];
        kb.send_frame(&frame).unwrap();

        kb.leave_direct(&hw).unwrap();

        check_golden("evision_320f_5016.hex", &writes.lock().unwrap());
    }

    /// Ao conectar com o teclado no modo Custom, restaura o efeito salvo.
    #[test]
    fn connect_restores_after_crash() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockEvision::new(Arc::clone(&writes));
        mock.config[1] = MODE_CUSTOM;
        let mut kb = Evision::with_io(Box::new(mock), 0);
        let saved = HwMode { mode: 6, ..HwMode::default() };
        assert_eq!(kb.connect(&saved).unwrap(), Some(saved));
        assert_eq!(kb.read_hw_mode().unwrap(), saved);
    }

    /// Modelos com a numeração do OpenRGB: fixo (5) vai como 6, custom como 0x14.
    #[test]
    fn openrgb_numbering() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut kb = Evision::with_io(Box::new(MockEvision::new(Arc::clone(&writes))), 1);
        kb.check_capabilities().unwrap();
        kb.set_hw_mode(&HwMode { mode: MODE_STATIC, ..HwMode::default() }).unwrap();
        kb.enter_direct(&HwMode::default()).unwrap();
        let config_writes: Vec<u8> =
            writes.lock().unwrap().iter().filter(|p| p[3] == cmd::WRITE_CONFIG && p[5] == 1).map(|p| p[8]).collect();
        assert_eq!(config_writes, vec![6, 0x14]);
    }

    /// Firmware que não responde `AA 55` é recusado.
    #[test]
    fn rejects_non_v2() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut mock = MockEvision::new(Arc::clone(&writes));
        mock.capabilities = vec![0; 7];
        let mut kb = Evision::with_io(Box::new(mock), 1);
        assert!(kb.check_capabilities().is_err());
    }

    fn onboard_kb(writes: &Arc<Mutex<Vec<Vec<u8>>>>) -> Evision {
        let mut kb = Evision::with_io(Box::new(MockEvision::new(Arc::clone(writes))), 0);
        kb.onboard = true;
        kb
    }

    /// Lê e grava teclas e macros e pede o reset de fábrica. Os bytes ficam
    /// guardados em `testdata/evision_onboard.hex`.
    #[test]
    fn golden_onboard() {
        use crate::onboard::{encode_macros, KeyAction, MacroInput, MacroStep};
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut kb = onboard_kb(&writes);
        let ob = kb.onboard().unwrap();

        let mut keys = ob.read_keys().unwrap();
        assert_eq!(KeyAction::decode(keys[110]), KeyAction::Fn);
        keys[64] = KeyAction::Media(0x00CD).encode();
        ob.write_keys(&keys).unwrap();
        assert_eq!(ob.read_keys().unwrap(), keys);

        assert_eq!(crate::onboard::decode_macros(&ob.read_macros().unwrap()).unwrap(), Vec::<crate::onboard::Macro>::new());
        let m = vec![
            MacroStep { input: MacroInput::Key(0x04), press: true, delay_ms: 20 },
            MacroStep { input: MacroInput::Key(0x04), press: false, delay_ms: 20 },
        ];
        // Duas macros, a segunda longa o bastante para ocupar mais de um pacote.
        let block = encode_macros(&[m.clone(), m.repeat(10)]);
        assert!(block.len() > MAX_PAYLOAD);
        ob.write_macros(&block).unwrap();
        assert_eq!(ob.read_macros().unwrap(), block);

        ob.factory_reset().unwrap();
        check_golden("evision_onboard.hex", &writes.lock().unwrap());
    }

    /// Escrita da tabela: início, 7 pacotes de até 56 bytes (a tabela tem
    /// 378) com o deslocamento de cada um, e fim.
    #[test]
    fn key_table_packets() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut kb = onboard_kb(&writes);
        let ob = kb.onboard().unwrap();
        let keys = ob.read_keys().unwrap();
        writes.lock().unwrap().clear();
        ob.write_keys(&keys).unwrap();
        let w = writes.lock().unwrap();
        let cmds: Vec<(u8, u8, u16)> = w.iter().map(|p| (p[3], p[4], u16::from_le_bytes([p[5], p[6]]))).collect();
        let mut expected = vec![(cmd::BEGIN, 0, 0)];
        for i in 0..7u16 {
            expected.push((cmd::WRITE_KEYS, if i == 6 { 42 } else { 56 }, i * 56));
        }
        expected.push((cmd::END, 0, 0));
        assert_eq!(cmds, expected);
        // Checksum: soma dos bytes 3..64.
        for p in w.iter() {
            let sum = p[3..].iter().fold(0u16, |a, &b| a.wrapping_add(b as u16));
            assert_eq!(u16::from_le_bytes([p[1], p[2]]), sum);
        }
    }

    #[test]
    fn onboard_only_on_known_models() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut kb = Evision::with_io(Box::new(MockEvision::new(Arc::clone(&writes))), 1);
        assert!(kb.onboard().is_none());
        kb.onboard = true;
        kb.map_size = 100;
        assert!(kb.onboard().is_none(), "matriz diferente de 6 x 21");
        let mut bad = onboard_kb(&writes);
        assert!(bad.onboard().unwrap().write_macros(&[0xAA, 0x55, 99, 0]).is_err());
        assert!(bad.onboard().unwrap().write_keys(&[[0; 3]; 3]).is_err());
    }

    #[test]
    fn recognizes_models_outside_the_usual_vendors() {
        let info = HidInfo { path: "x".into(), vid: 0x0C45, pid: 0x8501, usage_page: 0xFF1C, usage: 0x92, product: String::new(), release: 0 };
        let e = recognize(&info).unwrap();
        assert!(!e.tested);
        assert_eq!(e.id, "evision:0c45:8501");
        // Outros dispositivos do mesmo fabricante de chip não entram.
        assert!(recognize(&HidInfo { pid: 0x7000, ..info }).is_none());
    }

    #[test]
    fn recognizes_only_v2_control_interface() {
        let info = |vid, pid, page| HidInfo { path: "x".into(), vid, pid, usage_page: page, usage: 0x92, product: String::new(), release: 0 };
        let e = recognize(&info(0x320F, 0x5016, 0xFF1C)).unwrap();
        assert!(e.tested);
        assert_eq!(e.id, "evision:320f:5016");
        assert!(recognize(&info(0x320F, 0x5016, 0x0001)).is_none()); // interface de teclado
        assert!(recognize(&info(0x320F, 0x5000, 0xFF1C)).is_none()); // EVision V1
        assert!(recognize(&info(0x046D, 0xC33F, 0xFF1C)).is_none()); // outro fabricante
        assert!(!recognize(&info(0x320F, 0x5099, 0xFF1C)).unwrap().tested);
    }
}
