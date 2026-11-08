//! Captura o som que está tocando no PC (monitor da saída padrão do
//! PipeWire, via `pw-record`) e calcula as faixas de frequência com FFT.
//!
//! A captura só roda enquanto `wanted` estiver ligado, ou seja, enquanto
//! um efeito de música estiver ativo.

use crate::device::COLS;
use crate::effects::AudioData;
use rustfft::{num_complex::Complex, FftPlanner};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const RATE: f32 = 48_000.0;
const FFT_SIZE: usize = 2048;
/// Amostras lidas por vez (~21 ms), uma análise a cada bloco.
const HOP: usize = 1024;
const MIN_HZ: f32 = 40.0;
const MAX_HZ: f32 = 16_000.0;
/// Faixa dinâmica, em dB, abaixo do pico recente.
const RANGE_DB: f32 = 45.0;

pub fn spawn(shared: Arc<Mutex<AudioData>>, wanted: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("audio".into())
        .spawn(move || loop {
            if !wanted.load(Ordering::Relaxed) {
                *shared.lock().unwrap() = AudioData::default();
                thread::sleep(Duration::from_millis(200));
                continue;
            }
            match start_capture() {
                Ok(mut child) => {
                    analyze(&mut child, &shared, &wanted);
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(e) => eprintln!("áudio: não consegui iniciar o pw-record: {e}"),
            }
            thread::sleep(Duration::from_millis(500));
        })
        .expect("não consegui criar a thread de áudio");
}

fn start_capture() -> std::io::Result<Child> {
    Command::new("pw-record")
        .args(["-P", "{ stream.capture.sink=true node.name=keyglow }"])
        .args(["--format", "f32", "--rate", "48000", "--channels", "1", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

fn analyze(child: &mut Child, shared: &Mutex<AudioData>, wanted: &AtomicBool) {
    let Some(mut stdout) = child.stdout.take() else { return };
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        .collect();
    let edges = band_edges();

    let mut samples = vec![0.0f32; FFT_SIZE];
    let mut raw = vec![0u8; HOP * 4];
    let mut spectrum = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut smooth = [0.0f32; COLS];
    let mut peak_db = -30.0f32;

    while wanted.load(Ordering::Relaxed) {
        if stdout.read_exact(&mut raw).is_err() {
            return; // pw-record terminou; o loop principal tenta de novo
        }
        // Desliza a janela e acrescenta as amostras novas.
        samples.copy_within(HOP.., 0);
        for (dst, b) in samples[FFT_SIZE - HOP..].iter_mut().zip(raw.chunks_exact(4)) {
            *dst = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }

        for (i, c) in spectrum.iter_mut().enumerate() {
            *c = Complex::new(samples[i] * window[i], 0.0);
        }
        fft.process(&mut spectrum);

        let mut band_db = [0.0f32; COLS];
        for (b, db) in band_db.iter_mut().enumerate() {
            let (lo, hi) = edges[b];
            let mag = spectrum[lo..hi].iter().map(|c| c.norm()).fold(0.0, f32::max);
            *db = 20.0 * (mag / FFT_SIZE as f32 + 1e-9).log10();
        }

        // Ganho automático: acompanha o pico recente, para funcionar
        // com qualquer volume.
        let frame_peak = band_db.iter().copied().fold(f32::MIN, f32::max);
        peak_db = if frame_peak > peak_db { frame_peak } else { (peak_db - 0.03).max(-60.0) };

        for (s, db) in smooth.iter_mut().zip(band_db) {
            let v = ((db - (peak_db - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0).powf(1.5);
            // Sobe rápido e desce devagar, como um VU.
            *s += (v - *s) * if v > *s { 0.6 } else { 0.12 };
        }

        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / FFT_SIZE as f32).sqrt();
        let mut data = shared.lock().unwrap();
        data.bands = smooth;
        data.bass = smooth[..4].iter().sum::<f32>() / 4.0;
        data.level = (rms * 4.0).min(1.0);
    }
}

/// Divide o espectro em `COLS` faixas em escala logarítmica (índices dos bins).
fn band_edges() -> Vec<(usize, usize)> {
    let bin_hz = RATE / FFT_SIZE as f32;
    let ratio = MAX_HZ / MIN_HZ;
    (0..COLS)
        .map(|b| {
            let f0 = MIN_HZ * ratio.powf(b as f32 / COLS as f32);
            let f1 = MIN_HZ * ratio.powf((b + 1) as f32 / COLS as f32);
            let lo = (f0 / bin_hz).floor() as usize;
            let hi = ((f1 / bin_hz).ceil() as usize).max(lo + 1);
            (lo, hi.min(FFT_SIZE / 2))
        })
        .collect()
}
