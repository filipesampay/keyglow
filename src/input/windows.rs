//! Windows: hook global de teclado de baixo nível (`WH_KEYBOARD_LL`).
//!
//! Funciona com a janela fora de foco. Diferente do Linux, o hook vê todos
//! os teclados conectados, não só este. Os scancodes do Windows (set 1) são
//! convertidos para os códigos do Linux, que é o que a calibração salva.

use super::InputState;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// O callback do hook é uma função "solta" do Windows, então o estado fica global.
static STATE: OnceLock<Arc<Mutex<InputState>>> = OnceLock::new();

const VK_PAUSE: u32 = 0x13;
const VK_NUMLOCK: u32 = 0x90;
const VK_VOLUME_MUTE: u32 = 0xAD;
const VK_VOLUME_DOWN: u32 = 0xAE;
const VK_VOLUME_UP: u32 = 0xAF;

/// Converte (scancode, estendida?, virtual key) para o código evdev do Linux.
/// Nas teclas comuns, o scancode do set 1 é o próprio código do Linux.
fn to_linux_code(scan: u32, extended: bool, vk: u32) -> Option<u16> {
    let code = match vk {
        VK_VOLUME_MUTE => 113,
        VK_VOLUME_DOWN => 114,
        VK_VOLUME_UP => 115,
        VK_PAUSE => 119,
        VK_NUMLOCK => 69,
        _ if extended => match scan {
            0x1C => 96,  // Enter do numérico
            0x1D => 97,  // Ctrl direito
            0x35 => 98,  // / do numérico
            0x37 => 99,  // PrintScreen
            0x38 => 100, // AltGr
            0x47 => 102, // Home
            0x48 => 103, // ↑
            0x49 => 104, // PgUp
            0x4B => 105, // ←
            0x4D => 106, // →
            0x4F => 107, // End
            0x50 => 108, // ↓
            0x51 => 109, // PgDn
            0x52 => 110, // Insert
            0x53 => 111, // Delete
            0x5B => 125, // Super esquerdo
            0x5C => 126, // Super direito
            0x5D => 127, // Menu
            _ => return None,
        },
        _ => match scan {
            0x73 => 89,  // /? do ABNT2
            0x7E => 121, // . do numérico ABNT2
            1..=0x58 => scan as u16,
            _ => return None,
        },
    };
    Some(code)
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(state) = STATE.get()
    {
        // SAFETY: para WH_KEYBOARD_LL, `lparam` aponta para um KBDLLHOOKSTRUCT válido.
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(wparam as u32, WM_KEYUP | WM_SYSKEYUP);
        // Ignora teclas simuladas por software e o Ctrl falso que o Windows
        // gera junto com o AltGr (scancode com o bit 0x200).
        let synthetic = kb.flags & LLKHF_INJECTED != 0 || kb.scanCode & 0x200 != 0;
        if (down || up)
            && !synthetic
            && let Some(key) = to_linux_code(kb.scanCode, kb.flags & LLKHF_EXTENDED != 0, kb.vkCode)
        {
            // As travas desse estado duram microssegundos.
            state.lock().unwrap().record(key, down);
        }
    }
    // SAFETY: repassa o evento para os outros hooks, como a API exige.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

pub fn spawn(state: Arc<Mutex<InputState>>) {
    let _ = STATE.set(Arc::clone(&state));
    thread::Builder::new()
        .name("input-hook".into())
        .spawn(move || {
            // SAFETY: chamadas Win32 com argumentos válidos; o hook pertence
            // a esta thread, que mantém o loop de mensagens que ele exige.
            unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                let handle = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module, 0);
                if handle.is_null() {
                    state.lock().unwrap().error = Some("não consegui instalar o hook de teclado do Windows".into());
                    return;
                }
                state.lock().unwrap().devices = 1;
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
            }
        })
        .expect("não consegui criar a thread de entrada");
}
