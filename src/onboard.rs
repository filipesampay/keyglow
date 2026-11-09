//! Memória do teclado (firmware EVision V2): o que cada tecla faz e as macros.
//!
//! Este módulo só converte dados; quem conversa com o teclado é o driver
//! (`drivers::evision`).
//!
//! # Mapa de teclas
//!
//! O firmware guarda 126 posições (matriz 6 x 21), 3 bytes cada, **coluna por
//! coluna** (posição `coluna * 6 + linha`). Aqui a tabela fica linha por linha
//! (`linha * 21 + coluna`), a mesma numeração dos LEDs no keyglow.
//!
//! Cada posição:
//!
//! | Bytes          | Significado                                       |
//! |----------------|---------------------------------------------------|
//! | `20 mm uu`     | tecla `uu` (uso HID) com os modificadores `mm`    |
//! | `20 00 00`     | nada (tecla desativada)                           |
//! | `30 lo hi`     | tecla multimídia (uso HID da página Consumer)     |
//! | `10 bb 00`     | botão do mouse (`bb` = 1 << (botão - 1))          |
//! | `40 nn 00`     | energia: 1 desligar, 2 dormir, 3 acordar          |
//! | `71 mm nn`     | macro `mm`, executada `nn` vezes                  |
//! | `70 mm 01`     | macro `mm` repetindo até a tecla ser apertada de novo |
//! | `70 mm 02`     | macro `mm` repetindo enquanto a tecla está apertada |
//! | `a0 01 00`     | tecla Fn (nunca é alterada pelo keyglow)          |
//!
//! # Macros
//!
//! Um bloco único com todas as macros:
//!
//! ```text
//! cabeçalho (16 bytes): aa 55 | tamanho total (u16) | quantidade (u16) | zeros
//! deslocamentos: um u16 por macro, a partir do início do bloco
//! cada macro: número de passos (u32) e os passos, 4 bytes cada:
//!     espera depois do passo em ms (u16) | ação | código
//! ```
//!
//! Ação: bit 7 = apertar (sem ele, soltar); resto = 10 tecla (código = uso
//! HID), 9 modificador (código = bit do modificador), 1 botão do mouse
//! (código = 1 << (botão - 1)). Inteiros em little-endian.

use serde::{Deserialize, Serialize};

pub const ROWS: usize = 6;
pub const COLS: usize = 21;
/// Posições da tabela de teclas.
pub const SLOTS: usize = ROWS * COLS;
/// Tamanho da tabela de teclas no firmware.
pub const TABLE_LEN: usize = SLOTS * 3;
/// Quantas macros o keyglow gerencia.
pub const MAX_MACROS: usize = 10;
/// Passos por macro (cada tecla apertada e solta conta dois).
pub const MAX_MACRO_STEPS: usize = 70;
/// Maior bloco de macros que o keyglow lê ou grava.
pub const MAX_MACRO_BLOCK: usize = 16 + MAX_MACROS * (2 + 4 + MAX_MACRO_STEPS * 4);

pub type Cell = [u8; 3];

const MACRO_MAGIC: [u8; 2] = [0xAA, 0x55];
const MACRO_HEADER: usize = 16;

/// Tabela do firmware (coluna por coluna) -> linha por linha.
pub fn table_from_firmware(raw: &[u8]) -> Option<Vec<Cell>> {
    if raw.len() < TABLE_LEN {
        return None;
    }
    let mut cells = vec![[0u8; 3]; SLOTS];
    for col in 0..COLS {
        for row in 0..ROWS {
            let fw = (col * ROWS + row) * 3;
            cells[row * COLS + col] = [raw[fw], raw[fw + 1], raw[fw + 2]];
        }
    }
    Some(cells)
}

/// Linha por linha -> tabela do firmware (coluna por coluna).
pub fn table_to_firmware(cells: &[Cell]) -> Vec<u8> {
    let mut raw = vec![0u8; TABLE_LEN];
    for col in 0..COLS {
        for row in 0..ROWS {
            let fw = (col * ROWS + row) * 3;
            raw[fw..fw + 3].copy_from_slice(&cells[row * COLS + col]);
        }
    }
    raw
}

/// Tabela em texto hexadecimal (para salvar na configuração).
pub fn cells_to_hex(cells: &[Cell]) -> String {
    cells.iter().flatten().map(|b| format!("{b:02x}")).collect()
}

pub fn cells_from_hex(text: &str) -> Option<Vec<Cell>> {
    if text.len() != TABLE_LEN * 2 || !text.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect();
    Some(bytes?.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Como uma macro é disparada pela tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroRun {
    /// Executa N vezes (1 a 255).
    Times(u8),
    /// Repete até a tecla ser apertada de novo.
    UntilPressedAgain,
    /// Repete enquanto a tecla estiver apertada.
    WhileHeld,
}

/// O que uma tecla faz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Disabled,
    /// Uso HID da página teclado, com modificadores (bits HID: 1 Ctrl, 2 Shift,
    /// 4 Alt, 8 Win; os quatro seguintes, os do lado direito).
    Key { mods: u8, usage: u8 },
    /// Uso HID da página Consumer.
    Media(u16),
    /// Botão do mouse, 1 a 5.
    Mouse(u8),
    /// 1 desligar, 2 dormir, 3 acordar.
    Power(u8),
    Macro { slot: u8, run: MacroRun },
    Fn,
    /// Conteúdo que o keyglow não conhece; preservado como está.
    Other(Cell),
}

impl KeyAction {
    pub fn decode(c: Cell) -> Self {
        match c {
            [0x20, 0, 0] => KeyAction::Disabled,
            [0x20, mods, usage] => KeyAction::Key { mods, usage },
            [0x30, lo, hi] => KeyAction::Media(u16::from_le_bytes([lo, hi])),
            [0x10, b, 0] if b.is_power_of_two() && b <= 0x10 => KeyAction::Mouse(b.trailing_zeros() as u8 + 1),
            [0x40, n @ 1..=3, 0] => KeyAction::Power(n),
            [0x71, slot, n] if n > 0 => KeyAction::Macro { slot, run: MacroRun::Times(n) },
            [0x70, slot, 1] => KeyAction::Macro { slot, run: MacroRun::UntilPressedAgain },
            [0x70, slot, 2] => KeyAction::Macro { slot, run: MacroRun::WhileHeld },
            [0xA0, ..] => KeyAction::Fn,
            _ => KeyAction::Other(c),
        }
    }

    pub fn encode(self) -> Cell {
        match self {
            KeyAction::Disabled => [0x20, 0, 0],
            KeyAction::Key { mods, usage } => [0x20, mods, usage],
            KeyAction::Media(u) => {
                let [lo, hi] = u.to_le_bytes();
                [0x30, lo, hi]
            }
            KeyAction::Mouse(b) => [0x10, 1 << (b.clamp(1, 5) - 1), 0],
            KeyAction::Power(n) => [0x40, n.clamp(1, 3), 0],
            KeyAction::Macro { slot, run: MacroRun::Times(n) } => [0x71, slot, n.max(1)],
            KeyAction::Macro { slot, run: MacroRun::UntilPressedAgain } => [0x70, slot, 1],
            KeyAction::Macro { slot, run: MacroRun::WhileHeld } => [0x70, slot, 2],
            KeyAction::Fn => [0xA0, 1, 0],
            KeyAction::Other(c) => c,
        }
    }

}

/// Aplica alterações (posição, conteúdo novo) a uma tabela lida do teclado.
/// A tecla Fn não pode ser alterada nem criada: sem ela não há como voltar
/// às funções do teclado. Devolve quantas posições mudaram.
pub fn apply_key_edits(table: &mut [Cell], edits: &[(usize, Cell)]) -> usize {
    let mut changed = 0;
    for &(slot, cell) in edits {
        let Some(current) = table.get_mut(slot) else { continue };
        if KeyAction::decode(*current) == KeyAction::Fn || KeyAction::decode(cell) == KeyAction::Fn {
            continue;
        }
        if *current != cell {
            *current = cell;
            changed += 1;
        }
    }
    changed
}

/// Posição da tabela que, na tabela original, digita o uso HID `usage`.
/// Serve para achar a tecla certa mesmo quando a posição do LED dela é outra.
pub fn slot_of_usage(original: &[Cell], usage: u8) -> Option<usize> {
    original.iter().position(|&c| c == [0x20, 0, usage])
}

/// O que um passo de macro aperta ou solta.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroInput {
    /// Uso HID da página teclado (0xE0..0xE7 = modificadores).
    Key(u8),
    /// Botão do mouse, 1 a 5.
    Mouse(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStep {
    pub input: MacroInput,
    /// `true` = apertar, `false` = soltar.
    pub press: bool,
    /// Espera depois deste passo, em milissegundos.
    pub delay_ms: u16,
}

pub type Macro = Vec<MacroStep>;

const ACT_PRESS: u8 = 0x80;
const ACT_MOUSE: u8 = 1;
const ACT_MODIFIER: u8 = 9;
const ACT_KEY: u8 = 10;

impl MacroStep {
    fn encode(self) -> [u8; 4] {
        let (kind, code) = match self.input {
            MacroInput::Key(u @ 0xE0..=0xE7) => (ACT_MODIFIER, 1 << (u - 0xE0)),
            MacroInput::Key(u) => (ACT_KEY, u),
            MacroInput::Mouse(b) => (ACT_MOUSE, 1 << (b.clamp(1, 5) - 1)),
        };
        let [lo, hi] = self.delay_ms.to_le_bytes();
        [lo, hi, kind | if self.press { ACT_PRESS } else { 0 }, code]
    }

    fn decode(b: &[u8]) -> Option<Self> {
        let press = b[2] & ACT_PRESS != 0;
        let input = match (b[2] & !ACT_PRESS, b[3]) {
            (ACT_KEY, u) => MacroInput::Key(u),
            (ACT_MODIFIER, bit) if bit.is_power_of_two() => MacroInput::Key(0xE0 + bit.trailing_zeros() as u8),
            (ACT_MOUSE, bit) if bit.is_power_of_two() && bit <= 0x10 => MacroInput::Mouse(bit.trailing_zeros() as u8 + 1),
            _ => return None,
        };
        Some(Self { input, press, delay_ms: u16::from_le_bytes([b[0], b[1]]) })
    }
}

/// Monta o bloco de macros. Macros vazias no fim não são gravadas; as do
/// meio continuam ocupando a posição delas, para as teclas que apontam para
/// as seguintes não mudarem de macro.
pub fn encode_macros(macros: &[Macro]) -> Vec<u8> {
    let count = macros.iter().rposition(|m| !m.is_empty()).map_or(0, |i| i + 1).min(MAX_MACROS);
    let bodies: Vec<Vec<u8>> = macros[..count]
        .iter()
        .map(|m| {
            let steps = &m[..m.len().min(MAX_MACRO_STEPS)];
            let mut body = (steps.len() as u32).to_le_bytes().to_vec();
            for s in steps {
                body.extend_from_slice(&s.encode());
            }
            body
        })
        .collect();

    let mut out = vec![0u8; MACRO_HEADER];
    out[..2].copy_from_slice(&MACRO_MAGIC);
    out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
    let mut offset = MACRO_HEADER + count * 2;
    for body in &bodies {
        out.extend_from_slice(&(offset as u16).to_le_bytes());
        offset += body.len();
    }
    for body in &bodies {
        out.extend_from_slice(body);
    }
    let total = out.len() as u16;
    out[2..4].copy_from_slice(&total.to_le_bytes());
    out
}

/// Tamanho total do bloco, lido do cabeçalho; `None` se não há macros gravadas.
pub fn macro_block_len(header: &[u8]) -> Option<usize> {
    if header.len() < MACRO_HEADER || header[..2] != MACRO_MAGIC {
        return None;
    }
    let len = u16::from_le_bytes([header[2], header[3]]) as usize;
    (MACRO_HEADER..=MAX_MACRO_BLOCK).contains(&len).then_some(len)
}

/// Lê o bloco de macros. Bloco vazio ou sem a assinatura = nenhuma macro.
pub fn decode_macros(block: &[u8]) -> Result<Vec<Macro>, String> {
    let Some(len) = macro_block_len(block) else { return Ok(Vec::new()) };
    if block.len() < len {
        return Err("bloco de macros incompleto".into());
    }
    let block = &block[..len];
    let count = u16::from_le_bytes([block[4], block[5]]) as usize;
    if count > MAX_MACROS || MACRO_HEADER + count * 2 > len {
        return Err(format!("bloco de macros com {count} macros; o keyglow lida com até {MAX_MACROS}"));
    }
    let mut macros = Vec::with_capacity(count);
    for i in 0..count {
        let at = MACRO_HEADER + i * 2;
        let start = u16::from_le_bytes([block[at], block[at + 1]]) as usize;
        let steps_at = start + 4;
        if steps_at > len {
            return Err(format!("macro {} fora do bloco", i + 1));
        }
        let n = u32::from_le_bytes(block[start..steps_at].try_into().unwrap()) as usize;
        if n > MAX_MACRO_STEPS || steps_at + n * 4 > len {
            return Err(format!("macro {} com tamanho inválido", i + 1));
        }
        let steps = block[steps_at..steps_at + n * 4]
            .chunks(4)
            .map(|b| MacroStep::decode(b).ok_or_else(|| format!("macro {} tem um passo desconhecido", i + 1)))
            .collect::<Result<Macro, String>>()?;
        macros.push(steps);
    }
    Ok(macros)
}

/// Teclas multimídia conhecidas (uso HID Consumer, nome).
pub const MEDIA_KEYS: &[(u16, &str)] = &[
    (0x00CD, "Tocar/pausar"),
    (0x00B5, "Próxima faixa"),
    (0x00B6, "Faixa anterior"),
    (0x00B7, "Parar"),
    (0x00E2, "Mudo"),
    (0x00E9, "Volume +"),
    (0x00EA, "Volume -"),
    (0x0183, "Player de mídia"),
    (0x018A, "E-mail"),
    (0x0192, "Calculadora"),
    (0x0194, "Meu computador"),
    (0x0221, "Pesquisar"),
    (0x0223, "Navegador"),
    (0x0224, "Voltar (navegador)"),
    (0x0225, "Avançar (navegador)"),
    (0x0227, "Recarregar (navegador)"),
    (0x022A, "Favoritos"),
];

pub const MOUSE_BUTTONS: &[(u8, &str)] =
    &[(1, "Botão esquerdo"), (2, "Botão direito"), (3, "Botão do meio"), (4, "Voltar"), (5, "Avançar")];

pub const POWER_ACTIONS: &[(u8, &str)] = &[(1, "Desligar"), (2, "Dormir"), (3, "Acordar")];

/// Modificadores (bit, nome) na ordem dos bits HID.
pub const MODIFIERS: &[(u8, &str)] = &[
    (0x01, "Ctrl"),
    (0x02, "Shift"),
    (0x04, "Alt"),
    (0x08, "Win"),
    (0x10, "Ctrl dir."),
    (0x20, "Shift dir."),
    (0x40, "AltGr"),
    (0x80, "Win dir."),
];

pub fn media_name(usage: u16) -> String {
    MEDIA_KEYS.iter().find(|(u, _)| *u == usage).map_or_else(|| format!("Multimídia 0x{usage:03x}"), |(_, n)| n.to_string())
}

fn lookup(table: &[(u8, &str)], value: u8, fallback: &str) -> String {
    table.iter().find(|(v, _)| *v == value).map_or_else(|| format!("{fallback} {value}"), |(_, n)| n.to_string())
}

pub fn mouse_name(button: u8) -> String {
    lookup(MOUSE_BUTTONS, button, "Botão")
}

pub fn power_name(n: u8) -> String {
    lookup(POWER_ACTIONS, n, "Energia")
}

pub fn mods_prefix(mods: u8) -> String {
    MODIFIERS.iter().filter(|(bit, _)| mods & bit != 0).map(|(_, n)| format!("{n}+")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_stored_column_by_column() {
        let mut raw = vec![0u8; TABLE_LEN];
        // Segunda posição do firmware = linha 1, coluna 0 = posição 21 no keyglow.
        raw[3..6].copy_from_slice(&[0x20, 0, 0x2B]);
        // Posição 6 do firmware = linha 0, coluna 1 = posição 1.
        raw[18..21].copy_from_slice(&[0x20, 0, 0x3A]);
        let cells = table_from_firmware(&raw).unwrap();
        assert_eq!(cells[21], [0x20, 0, 0x2B]);
        assert_eq!(cells[1], [0x20, 0, 0x3A]);
        assert_eq!(table_to_firmware(&cells), raw);
        assert!(table_from_firmware(&raw[..10]).is_none());
    }

    #[test]
    fn table_hex_round_trip() {
        let cells: Vec<Cell> = (0..SLOTS).map(|i| [0x20, i as u8, 0xFF]).collect();
        let hex = cells_to_hex(&cells);
        assert_eq!(hex.len(), TABLE_LEN * 2);
        assert_eq!(cells_from_hex(&hex), Some(cells));
        assert_eq!(cells_from_hex("abc"), None);
        assert_eq!(cells_from_hex(&"zz".repeat(TABLE_LEN)), None);
    }

    #[test]
    fn key_actions_round_trip() {
        let actions = [
            KeyAction::Disabled,
            KeyAction::Key { mods: 0, usage: 0x04 },
            KeyAction::Key { mods: 0x05, usage: 0x06 },
            KeyAction::Media(0x00CD),
            KeyAction::Media(0x0192),
            KeyAction::Mouse(1),
            KeyAction::Mouse(5),
            KeyAction::Power(2),
            KeyAction::Macro { slot: 3, run: MacroRun::Times(7) },
            KeyAction::Macro { slot: 0, run: MacroRun::UntilPressedAgain },
            KeyAction::Macro { slot: 9, run: MacroRun::WhileHeld },
            KeyAction::Fn,
            KeyAction::Other([0x55, 1, 2]),
        ];
        for a in actions {
            assert_eq!(KeyAction::decode(a.encode()), a, "{a:?}");
        }
        assert_eq!(KeyAction::Media(0x00CD).encode(), [0x30, 0xCD, 0x00]);
        assert_eq!(KeyAction::Media(0x0192).encode(), [0x30, 0x92, 0x01]);
        assert_eq!(KeyAction::Mouse(3).encode(), [0x10, 0x04, 0x00]);
        assert_eq!(KeyAction::Fn.encode(), [0xA0, 0x01, 0x00]);
        assert_eq!(KeyAction::decode([0xA0, 0x01, 0x00]), KeyAction::Fn);
        assert_eq!(KeyAction::Macro { slot: 2, run: MacroRun::Times(1) }.encode(), [0x71, 2, 1]);
    }

    #[test]
    fn edits_never_touch_fn() {
        let mut table = vec![[0x20, 0, 0x04]; SLOTS];
        table[110] = [0xA0, 1, 0];
        let edits = [(110, [0x20, 0, 0x05]), (3, [0xA0, 1, 0]), (4, [0x20, 0, 0x29]), (5, [0x20, 0, 0x04]), (999, [0; 3])];
        assert_eq!(apply_key_edits(&mut table, &edits), 1);
        assert_eq!(table[110], [0xA0, 1, 0]);
        assert_eq!(table[3], [0x20, 0, 0x04]);
        assert_eq!(table[4], [0x20, 0, 0x29]);
    }

    #[test]
    fn finds_key_by_original_content() {
        let mut table = vec![[0x20, 0, 0]; SLOTS];
        table[15] = [0x20, 0, 0x47];
        assert_eq!(slot_of_usage(&table, 0x47), Some(15));
        assert_eq!(slot_of_usage(&table, 0x48), None);
    }

    /// Apertar A, esperar 50 ms, soltar A, esperar 30 ms.
    #[test]
    fn macro_block_layout() {
        let m = vec![
            MacroStep { input: MacroInput::Key(0x04), press: true, delay_ms: 50 },
            MacroStep { input: MacroInput::Key(0x04), press: false, delay_ms: 30 },
        ];
        let block = encode_macros(std::slice::from_ref(&m));
        #[rustfmt::skip]
        let expected = [
            0xAA, 0x55, 30, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // cabeçalho
            18, 0,                                                  // deslocamento da macro 1
            2, 0, 0, 0,                                             // 2 passos
            50, 0, 0x8A, 0x04,                                      // aperta A, espera 50
            30, 0, 0x0A, 0x04,                                      // solta A, espera 30
        ];
        assert_eq!(block, expected);
        assert_eq!(decode_macros(&block).unwrap(), vec![m]);
    }

    #[test]
    fn macro_modifiers_and_mouse() {
        let m = vec![
            MacroStep { input: MacroInput::Key(0xE1), press: true, delay_ms: 0 },
            MacroStep { input: MacroInput::Mouse(1), press: true, delay_ms: 300 },
            MacroStep { input: MacroInput::Mouse(1), press: false, delay_ms: 0 },
            MacroStep { input: MacroInput::Key(0xE1), press: false, delay_ms: 0 },
        ];
        let block = encode_macros(std::slice::from_ref(&m));
        assert_eq!(&block[22..26], &[0, 0, 0x89, 0x02]);
        assert_eq!(&block[26..30], &[0x2C, 0x01, 0x81, 0x01]);
        assert_eq!(decode_macros(&block).unwrap(), vec![m]);
    }

    #[test]
    fn empty_macros_keep_their_slot() {
        let a = vec![MacroStep { input: MacroInput::Key(0x05), press: true, delay_ms: 1 }];
        let macros = vec![Vec::new(), a.clone(), Vec::new(), Vec::new()];
        let block = encode_macros(&macros);
        assert_eq!(block[4], 2);
        assert_eq!(decode_macros(&block).unwrap(), vec![Vec::new(), a]);
        // Nenhuma macro: só o cabeçalho.
        let none = encode_macros(&[]);
        assert_eq!(none.len(), 16);
        assert_eq!(decode_macros(&none).unwrap(), Vec::<Macro>::new());
    }

    #[test]
    fn rejects_broken_blocks() {
        assert_eq!(decode_macros(&[0xFF; 64]).unwrap(), Vec::<Macro>::new());
        let mut block = encode_macros(&[vec![MacroStep { input: MacroInput::Key(4), press: true, delay_ms: 0 }]]);
        block[18] = 200; // passos demais
        assert!(decode_macros(&block).is_err());
        let short = encode_macros(&[vec![MacroStep { input: MacroInput::Key(4), press: true, delay_ms: 0 }]]);
        assert!(decode_macros(&short[..20]).is_err());
    }
}
