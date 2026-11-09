//! Acesso ao canal de controle HID do teclado, uma implementação por sistema.
//!
//! Todas expõem a mesma API (`Port`):
//! - `Port::open()` acha a interface de controle (usage page 0xFF1C);
//! - `write(&buf)` envia um relatório de 64 bytes (com o report ID no byte 0);
//! - `read_timeout(&mut buf, ms)` lê um relatório; `Ok(0)` = tempo esgotado.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Port;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Port;

/// Usage page da interface de controle do firmware EVision.
pub const CONTROL_USAGE_PAGE: u16 = 0xFF1C;
