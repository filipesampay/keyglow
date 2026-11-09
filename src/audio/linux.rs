//! Linux: grava o monitor da saída padrão do PipeWire com `pw-record`.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const RATE: u32 = 48_000;

/// Entrega blocos de amostras mono a `on_samples` até `wanted` desligar.
pub fn capture(wanted: &AtomicBool, on_samples: &mut dyn FnMut(&[f32], u32)) -> io::Result<()> {
    let mut child = Command::new("pw-record")
        .args(["-P", "{ stream.capture.sink=true node.name=keyglow }"])
        .args(["--format", "f32", "--rate", &RATE.to_string(), "--channels", "1", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("não consegui iniciar o pw-record: {e}")))?;
    let mut stdout = child.stdout.take().expect("stdout configurado como pipe");

    let mut raw = vec![0u8; 1024 * 4];
    let mut samples = vec![0.0f32; 1024];
    let result = loop {
        if !wanted.load(Ordering::Relaxed) {
            break Ok(());
        }
        if let Err(e) = stdout.read_exact(&mut raw) {
            break Err(e);
        }
        for (dst, b) in samples.iter_mut().zip(raw.chunks_exact(4)) {
            *dst = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        on_samples(&samples, RATE);
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}
