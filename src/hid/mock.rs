//! Teclado EVision V2 de mentira, para testes: guarda tudo o que foi escrito
//! e responde como o firmware (memória de configuração e eco dos comandos).

use super::HidIo;
use std::io;
use std::sync::{Arc, Mutex};

pub struct MockEvision {
    pub writes: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Memória de configuração (perfil atual em 0x00, perfis a partir de 0x01).
    pub config: Vec<u8>,
    /// Resposta ao comando 0x03 (capacidades).
    pub capabilities: Vec<u8>,
    pending: Option<[u8; 64]>,
}

impl MockEvision {
    pub fn new(writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        let mut config = vec![0u8; 0x200];
        // Perfil 0 com o efeito "fixo" vermelho, brilho 4, velocidade 3.
        config[1..9].copy_from_slice(&[5, 4, 3, 0, 0, 255, 0, 0]);
        Self { writes, config, capabilities: vec![0xAA, 0x55, 0, 0, 0, 126, 0], pending: None }
    }
}

impl HidIo for MockEvision {
    fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.writes.lock().unwrap().push(buf.to_vec());
        let mut resp = [0u8; 64];
        resp[..8].copy_from_slice(&buf[..8]);
        let (cmd, size, off) = (buf[3], buf[4] as usize, u16::from_le_bytes([buf[5], buf[6]]) as usize);
        match cmd {
            0x05 => resp[8..8 + size].copy_from_slice(&self.config[off..off + size]),
            0x06 => self.config[off..off + size].copy_from_slice(&buf[8..8 + size]),
            0x03 => {
                let n = size.min(self.capabilities.len());
                resp[8..8 + n].copy_from_slice(&self.capabilities[..n]);
            }
            _ => {}
        }
        self.pending = Some(resp);
        Ok(())
    }

    fn read_timeout(&mut self, buf: &mut [u8], _timeout_ms: i32) -> io::Result<usize> {
        match self.pending.take() {
            Some(resp) => {
                buf[..64].copy_from_slice(&resp);
                Ok(64)
            }
            None => Ok(0),
        }
    }
}

/// Compara (ou grava, com `KEYGLOW_UPDATE_GOLDEN=1`) uma sequência de
/// pacotes com o arquivo de referência em `testdata/`.
pub fn check_golden(name: &str, packets: &[Vec<u8>]) {
    let text: String = packets
        .iter()
        .map(|p| p.iter().map(|b| format!("{b:02x}")).collect::<String>() + "\n")
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name);
    if std::env::var_os("KEYGLOW_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("falta {}", path.display()));
    assert!(expected == text, "pacotes diferentes da referência {name}");
}
