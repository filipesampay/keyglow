//! Leitura do "report descriptor" HID: a descrição, feita pelo próprio
//! dispositivo, de quais relatórios ele aceita e do que há em cada um.
//!
//! Só o necessário para o keyglow: as coleções de topo (para saber o que
//! cada interface é), o report ID de cada relatório LampArray e o tamanho
//! dos relatórios de feature.

use std::collections::BTreeMap;

/// Item curto do descriptor: `key` = prefixo sem os 2 bits de tamanho.
struct Item {
    key: u8,
    data: u32,
    /// Quantos bytes de dados o item tinha (0, 1, 2 ou 4).
    len: usize,
}

fn items(desc: &[u8]) -> impl Iterator<Item = Item> + '_ {
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos < desc.len() {
            let prefix = desc[pos];
            if prefix == 0xFE {
                // Item longo: 0xFE, tamanho, tag, dados. Não usamos.
                let size = *desc.get(pos + 1)? as usize;
                pos += 3 + size;
                continue;
            }
            let len = match prefix & 0x03 {
                3 => 4,
                n => n as usize,
            };
            let bytes = desc.get(pos + 1..pos + 1 + len)?;
            let data = bytes.iter().rev().fold(0u32, |acc, &b| (acc << 8) | b as u32);
            pos += 1 + len;
            return Some(Item { key: prefix & 0xFC, data, len });
        }
        None
    })
}

const USAGE_PAGE: u8 = 0x04;
const REPORT_SIZE: u8 = 0x74;
const REPORT_ID: u8 = 0x84;
const REPORT_COUNT: u8 = 0x94;
const USAGE: u8 = 0x08;
const COLLECTION: u8 = 0xA0;
const END_COLLECTION: u8 = 0xC0;
const INPUT: u8 = 0x80;
const OUTPUT: u8 = 0x90;
const FEATURE: u8 = 0xB0;

/// (usage page, usage) de cada coleção de aplicação de topo.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // usada na enumeração do Linux
pub fn top_level_usages(desc: &[u8]) -> Vec<(u16, u16)> {
    let (mut page, mut usage, mut depth) = (0u16, 0u16, 0usize);
    let mut out = Vec::new();
    for it in items(desc) {
        match it.key {
            USAGE_PAGE => page = it.data as u16,
            USAGE => {
                usage = it.data as u16;
                if it.len == 4 {
                    page = (it.data >> 16) as u16;
                }
            }
            COLLECTION => {
                // Qualquer coleção de topo conta (normalmente é de aplicação).
                if depth == 0 {
                    out.push((page, usage));
                }
                depth += 1;
            }
            END_COLLECTION => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    out
}

/// Report ID de cada coleção da usage page `page` (chave = usage da coleção).
///
/// Segue o mesmo algoritmo do OpenRGB para LampArray: o report ID pode vir
/// antes ou depois do usage da coleção lógica.
pub fn report_ids_by_usage(desc: &[u8], page_wanted: u16) -> BTreeMap<u16, u8> {
    let (mut page, mut usage, mut report_id, mut collection_usage) = (0u16, 0u16, 0u8, 0u16);
    let mut out = BTreeMap::new();
    for it in items(desc) {
        match it.key {
            COLLECTION => {
                collection_usage = usage;
                if page == page_wanted && report_id != 0 {
                    out.insert(collection_usage, report_id);
                }
            }
            END_COLLECTION => {
                collection_usage = 0;
                report_id = 0;
            }
            USAGE_PAGE => page = it.data as u16,
            USAGE => {
                usage = it.data as u16;
                if it.len == 4 && (it.data >> 16) as u16 == page_wanted && report_id != 0 {
                    out.insert(usage, report_id);
                }
            }
            REPORT_ID => {
                report_id = it.data as u8;
                if page == page_wanted && collection_usage != 0 {
                    out.insert(collection_usage, report_id);
                }
            }
            _ => {}
        }
    }
    out
}

/// Tamanho em bytes (sem o report ID) de cada relatório de feature.
pub fn feature_report_sizes(desc: &[u8]) -> BTreeMap<u8, usize> {
    let (mut size, mut count, mut report_id) = (0u32, 0u32, 0u8);
    let mut bits: BTreeMap<u8, u32> = BTreeMap::new();
    for it in items(desc) {
        match it.key {
            REPORT_SIZE => size = it.data,
            REPORT_COUNT => count = it.data,
            REPORT_ID => report_id = it.data as u8,
            FEATURE => *bits.entry(report_id).or_default() += size * count,
            INPUT | OUTPUT => {}
            _ => {}
        }
    }
    bits.into_iter().map(|(id, b)| (id, b.div_ceil(8) as usize)).collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Descriptor de exemplo da Microsoft para um teclado LampArray
    /// (só a parte de iluminação), com os report IDs 1 a 6.
    pub const LAMPARRAY_SAMPLE: &[u8] = &[
        0x05, 0x59, 0x09, 0x01, 0xA1, 0x01, // LampArray
        // 1: LampArrayAttributesReport
        0x85, 0x01, 0x09, 0x02, 0xA1, 0x02,
        0x09, 0x03, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75, 0x10, 0x95, 0x01, 0xB1, 0x03,
        0x09, 0x04, 0x09, 0x05, 0x09, 0x06, 0x09, 0x07, 0x09, 0x08,
        0x15, 0x00, 0x27, 0xFF, 0xFF, 0xFF, 0x7F, 0x75, 0x20, 0x95, 0x05, 0xB1, 0x03,
        0xC0,
        // 2: LampAttributesRequestReport
        0x85, 0x02, 0x09, 0x20, 0xA1, 0x02,
        0x09, 0x21, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75, 0x10, 0x95, 0x01, 0xB1, 0x02,
        0xC0,
        // 3: LampAttributesResponseReport
        0x85, 0x03, 0x09, 0x22, 0xA1, 0x02,
        0x09, 0x21, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75, 0x10, 0x95, 0x01, 0xB1, 0x02,
        0x09, 0x23, 0x09, 0x24, 0x09, 0x25, 0x09, 0x27, 0x09, 0x26,
        0x15, 0x00, 0x27, 0xFF, 0xFF, 0xFF, 0x7F, 0x75, 0x20, 0x95, 0x05, 0xB1, 0x02,
        0x09, 0x28, 0x09, 0x29, 0x09, 0x2A, 0x09, 0x2B, 0x09, 0x2C, 0x09, 0x2D,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x06, 0xB1, 0x02,
        0xC0,
        // 4: LampMultiUpdateReport
        0x85, 0x04, 0x09, 0x50, 0xA1, 0x02,
        0x09, 0x03, 0x09, 0x55, 0x15, 0x00, 0x25, 0x08, 0x75, 0x08, 0x95, 0x02, 0xB1, 0x02,
        0x09, 0x21, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75, 0x10, 0x95, 0x08, 0xB1, 0x02,
        0x09, 0x51, 0x09, 0x52, 0x09, 0x53, 0x09, 0x54,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x20, 0xB1, 0x02,
        0xC0,
        // 5: LampRangeUpdateReport
        0x85, 0x05, 0x09, 0x60, 0xA1, 0x02,
        0x09, 0x55, 0x15, 0x00, 0x25, 0x01, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x02,
        0x09, 0x61, 0x09, 0x62, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75, 0x10, 0x95, 0x02, 0xB1, 0x02,
        0x09, 0x51, 0x09, 0x52, 0x09, 0x53, 0x09, 0x54,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x04, 0xB1, 0x02,
        0xC0,
        // 6: LampArrayControlReport
        0x85, 0x06, 0x09, 0x70, 0xA1, 0x02,
        0x09, 0x71, 0x15, 0x00, 0x25, 0x01, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x02,
        0xC0,
        0xC0,
    ];

    #[test]
    fn lamparray_sample() {
        assert_eq!(top_level_usages(LAMPARRAY_SAMPLE), vec![(0x59, 0x01)]);
        let ids = report_ids_by_usage(LAMPARRAY_SAMPLE, 0x59);
        assert_eq!(ids.get(&0x02), Some(&1));
        assert_eq!(ids.get(&0x20), Some(&2));
        assert_eq!(ids.get(&0x22), Some(&3));
        assert_eq!(ids.get(&0x50), Some(&4));
        assert_eq!(ids.get(&0x60), Some(&5));
        assert_eq!(ids.get(&0x70), Some(&6));
        let sizes = feature_report_sizes(LAMPARRAY_SAMPLE);
        assert_eq!(sizes[&1], 22);
        assert_eq!(sizes[&2], 2);
        assert_eq!(sizes[&3], 28);
        assert_eq!(sizes[&4], 50);
        assert_eq!(sizes[&6], 1);
    }

    #[test]
    fn keyboard_with_vendor_page() {
        // Teclado comum + coleção de fabricante 0xFF1C (como o EVision).
        let desc = [
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xC0, // Generic Desktop / Keyboard
            0x06, 0x1C, 0xFF, 0x09, 0x92, 0xA1, 0x01, 0xC0, // 0xFF1C / 0x92
        ];
        assert_eq!(top_level_usages(&desc), vec![(0x01, 0x06), (0xFF1C, 0x92)]);
    }
}
