//! Captura o som que está tocando no PC e calcula as faixas de frequência.
//!
//! A captura depende do sistema:
//! - Linux: monitor da saída padrão do PipeWire, via `pw-record` (`linux.rs`);
//! - Windows: "loopback" da saída padrão via WASAPI (`windows.rs`);
//! - macOS 14.6+: loopback da saída padrão via process tap do CoreAudio
//!   (mesmo `windows.rs`).
//!
//! Elas só entregam amostras mono em f32; a análise (FFT) fica aqui. A
//! captura só roda enquanto `wanted` estiver ligado, ou seja, enquanto um
//! efeito de música estiver ativo.

use crate::effects::{AudioData, BANDS};
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::capture;

#[cfg(any(windows, target_os = "macos"))]
mod windows;
#[cfg(any(windows, target_os = "macos"))]
use windows::capture;

const FFT_SIZE: usize = 2048;
/// Amostras novas por análise (~21 ms a 48 kHz).
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
            let mut analyzer: Option<Analyzer> = None;
            let result = capture(&wanted, &mut |samples, rate| {
                let a = analyzer.get_or_insert_with(|| Analyzer::new(rate));
                if a.rate != rate {
                    *a = Analyzer::new(rate);
                }
                a.push(samples, &shared);
            });
            if let Err(e) = result {
                eprintln!("áudio: {e}");
            }
            thread::sleep(Duration::from_millis(500));
        })
        .expect("não consegui criar a thread de áudio");
}

/// Recebe amostras em qualquer quantidade e roda uma FFT a cada `HOP`.
struct Analyzer {
    rate: u32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    edges: Vec<(usize, usize)>,
    /// Buffer circular com as últimas `FFT_SIZE` amostras; `pos` é a mais antiga.
    samples: Vec<f32>,
    pos: usize,
    fresh: usize,
    spectrum: Vec<Complex<f32>>,
    smooth: [f32; BANDS],
    peak_db: f32,
}

impl Analyzer {
    fn new(rate: u32) -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        Self {
            rate,
            fft: FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE),
            window,
            edges: band_edges(rate as f32),
            samples: vec![0.0; FFT_SIZE],
            pos: 0,
            fresh: 0,
            spectrum: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            smooth: [0.0; BANDS],
            peak_db: -30.0,
        }
    }

    fn push(&mut self, input: &[f32], shared: &Mutex<AudioData>) {
        for &x in input {
            self.samples[self.pos] = x;
            self.pos = (self.pos + 1) % FFT_SIZE;
            self.fresh += 1;
            if self.fresh >= HOP {
                self.fresh = 0;
                self.analyze(shared);
            }
        }
    }

    fn analyze(&mut self, shared: &Mutex<AudioData>) {
        for (i, c) in self.spectrum.iter_mut().enumerate() {
            let sample = self.samples[(self.pos + i) % FFT_SIZE];
            *c = Complex::new(sample * self.window[i], 0.0);
        }
        self.fft.process(&mut self.spectrum);

        let mut band_db = [0.0f32; BANDS];
        for (b, db) in band_db.iter_mut().enumerate() {
            let (lo, hi) = self.edges[b];
            let mag = self.spectrum[lo..hi].iter().map(|c| c.norm()).fold(0.0, f32::max);
            *db = 20.0 * (mag / FFT_SIZE as f32 + 1e-9).log10();
        }

        // Ganho automático: acompanha o pico recente, para funcionar com
        // qualquer volume.
        let frame_peak = band_db.iter().copied().fold(f32::MIN, f32::max);
        self.peak_db = if frame_peak > self.peak_db { frame_peak } else { (self.peak_db - 0.03).max(-60.0) };

        for (s, db) in self.smooth.iter_mut().zip(band_db) {
            let v = ((db - (self.peak_db - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0).powf(1.5);
            // Sobe rápido e desce devagar, como um VU.
            *s += (v - *s) * if v > *s { 0.6 } else { 0.12 };
        }

        let rms = (self.samples.iter().map(|x| x * x).sum::<f32>() / FFT_SIZE as f32).sqrt();
        let mut data = shared.lock().unwrap();
        data.bands = self.smooth;
        data.bass = self.smooth[..4].iter().sum::<f32>() / 4.0;
        data.level = (rms * 4.0).min(1.0);
    }
}

/// Divide o espectro em `BANDS` faixas em escala logarítmica (índices dos bins).
fn band_edges(rate: f32) -> Vec<(usize, usize)> {
    let bin_hz = rate / FFT_SIZE as f32;
    let ratio = MAX_HZ / MIN_HZ;
    (0..BANDS)
        .map(|b| {
            let f0 = MIN_HZ * ratio.powf(b as f32 / BANDS as f32);
            let f1 = MIN_HZ * ratio.powf((b + 1) as f32 / BANDS as f32);
            let lo = (f0 / bin_hz).floor() as usize;
            let hi = ((f1 / bin_hz).ceil() as usize).max(lo + 1);
            (lo.min(FFT_SIZE / 2 - 1), hi.min(FFT_SIZE / 2))
        })
        .collect()
}
