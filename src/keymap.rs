//! Conversões para os códigos de tecla do Linux (evdev), que são os que o
//! keyglow usa em todo lugar (calibração, efeitos, desenho do teclado).
//!
//! Servem para montar o mapa tecla -> LED sem calibração quando o próprio
//! dispositivo diz qual tecla fica em cada LED (LampArray e OpenRGB).

/// Código de uso HID da página "Keyboard/Keypad" (0x07) -> código evdev.
pub fn hid_usage_to_evdev(usage: u16) -> Option<u16> {
    // Letras A..Z (0x04..0x1D) não seguem ordem no evdev.
    const LETTERS: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    // 0x59..0x61 = numérico 1..9.
    const KEYPAD: [u16; 9] = [79, 80, 81, 75, 76, 77, 71, 72, 73];
    let code = match usage {
        0x04..=0x1D => LETTERS[(usage - 0x04) as usize],
        0x1E..=0x27 => usage - 0x1E + 2, // 1..9, 0
        0x28 => 28,                      // Enter
        0x29 => 1,                       // Esc
        0x2A => 14,                      // Backspace
        0x2B => 15,                      // Tab
        0x2C => 57,                      // Espaço
        0x2D => 12,
        0x2E => 13,
        0x2F => 26,
        0x30 => 27,
        0x31 | 0x32 => 43, // \ e # (ISO), mesma posição
        0x33 => 39,
        0x34 => 40,
        0x35 => 41,
        0x36 => 51,
        0x37 => 52,
        0x38 => 53,
        0x39 => 58,                      // Caps Lock
        0x3A..=0x43 => usage - 0x3A + 59, // F1..F10
        0x44 => 87,                      // F11
        0x45 => 88,                      // F12
        0x46 => 99,                      // Print Screen
        0x47 => 70,                      // Scroll Lock
        0x48 => 119,                     // Pause
        0x49 => 110,                     // Insert
        0x4A => 102,                     // Home
        0x4B => 104,                     // Page Up
        0x4C => 111,                     // Delete
        0x4D => 107,                     // End
        0x4E => 109,                     // Page Down
        0x4F => 106,                     // →
        0x50 => 105,                     // ←
        0x51 => 108,                     // ↓
        0x52 => 103,                     // ↑
        0x53 => 69,                      // Num Lock
        0x54 => 98,                      // numérico /
        0x55 => 55,                      // numérico *
        0x56 => 74,                      // numérico -
        0x57 => 78,                      // numérico +
        0x58 => 96,                      // numérico Enter
        0x59..=0x61 => KEYPAD[(usage - 0x59) as usize],
        0x62 => 82,  // numérico 0
        0x63 => 83,  // numérico , (ABNT2)
        0x64 => 86,  // \ ao lado do Shift (ISO/ABNT2)
        0x65 => 127, // Menu
        0x7F => 113, // Mudo
        0x80 => 115, // Volume +
        0x81 => 114, // Volume -
        0x85 => 121, // numérico . (ABNT2)
        0x87 => 89,  // /? (ABNT2)
        0xE0 => 29,  // Ctrl esquerdo
        0xE1 => 42,  // Shift esquerdo
        0xE2 => 56,  // Alt
        0xE3 => 125, // Super esquerdo
        0xE4 => 97,  // Ctrl direito
        0xE5 => 54,  // Shift direito
        0xE6 => 100, // AltGr
        0xE7 => 126, // Super direito
        _ => return None,
    };
    Some(code)
}

/// Nome de LED do OpenRGB (`"Key: A"`, `"Key: Left Shift"`...) -> código evdev.
pub fn openrgb_name_to_evdev(name: &str) -> Option<u16> {
    let key = name.strip_prefix("Key: ")?.trim();
    if let Some(n) = key.strip_prefix("F").and_then(|n| n.parse::<u16>().ok()) {
        return match n {
            1..=10 => Some(58 + n),
            11 => Some(87),
            12 => Some(88),
            _ => None,
        };
    }
    if let Some(rest) = key.strip_prefix("Number Pad ") {
        return Some(match rest {
            "0" => 82,
            "1" => 79,
            "2" => 80,
            "3" => 81,
            "4" => 75,
            "5" => 76,
            "6" => 77,
            "7" => 71,
            "8" => 72,
            "9" => 73,
            "/" => 98,
            "*" => 55,
            "-" => 74,
            "+" => 78,
            "." | "," => 83,
            "Enter" => 96,
            _ => return None,
        });
    }
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_uppercase() {
            return hid_usage_to_evdev(c as u16 - 'A' as u16 + 0x04);
        }
        if let Some(d) = c.to_digit(10) {
            return Some(if d == 0 { 11 } else { d as u16 + 1 });
        }
    }
    Some(match key {
        "Escape" => 1,
        "`" => 41,
        "-" => 12,
        "=" => 13,
        "Backspace" => 14,
        "Tab" => 15,
        "[" => 26,
        "]" => 27,
        "\\ (ANSI)" | "#" => 43,
        "Caps Lock" => 58,
        ";" => 39,
        "'" => 40,
        "Enter" => 28,
        "Left Shift" => 42,
        "\\ (ISO)" => 86,
        "," => 51,
        "." => 52,
        "/" => 53,
        "Right Shift" => 54,
        "Left Control" => 29,
        "Left Windows" => 125,
        "Left Alt" => 56,
        "Space" => 57,
        "Right Alt" => 100,
        "Right Windows" => 126,
        "Menu" => 127,
        "Right Control" => 97,
        "Print Screen" => 99,
        "Scroll Lock" => 70,
        "Pause/Break" => 119,
        "Insert" => 110,
        "Home" => 102,
        "Page Up" => 104,
        "Delete" => 111,
        "End" => 107,
        "Page Down" => 109,
        "Up Arrow" => 103,
        "Left Arrow" => 105,
        "Down Arrow" => 108,
        "Right Arrow" => 106,
        "Num Lock" => 69,
        "Media Mute" => 113,
        "Media Volume +" => 115,
        "Media Volume -" => 114,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_usages() {
        assert_eq!(hid_usage_to_evdev(0x04), Some(30)); // A
        assert_eq!(hid_usage_to_evdev(0x1D), Some(44)); // Z
        assert_eq!(hid_usage_to_evdev(0x1E), Some(2)); // 1
        assert_eq!(hid_usage_to_evdev(0x27), Some(11)); // 0
        assert_eq!(hid_usage_to_evdev(0x43), Some(68)); // F10
        assert_eq!(hid_usage_to_evdev(0x61), Some(73)); // numérico 9
        assert_eq!(hid_usage_to_evdev(0xE6), Some(100));
        assert_eq!(hid_usage_to_evdev(0x00), None);
    }

    #[test]
    fn openrgb_names() {
        assert_eq!(openrgb_name_to_evdev("Key: A"), Some(30));
        assert_eq!(openrgb_name_to_evdev("Key: 0"), Some(11));
        assert_eq!(openrgb_name_to_evdev("Key: 1"), Some(2));
        assert_eq!(openrgb_name_to_evdev("Key: F12"), Some(88));
        assert_eq!(openrgb_name_to_evdev("Key: Number Pad 7"), Some(71));
        assert_eq!(openrgb_name_to_evdev("Key: Left Shift"), Some(42));
        assert_eq!(openrgb_name_to_evdev("Logo"), None);
    }
}
