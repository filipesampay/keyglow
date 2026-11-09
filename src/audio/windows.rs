//! Windows: grava a saída padrão em modo "loopback" do WASAPI, via `cpal`.
//! (No WASAPI, abrir um dispositivo de saída como entrada liga o loopback.)

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Entrega blocos de amostras mono a `on_samples` até `wanted` desligar.
pub fn capture(wanted: &AtomicBool, on_samples: &mut dyn FnMut(&[f32], u32)) -> io::Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nenhuma saída de áudio padrão"))?;
    let supported = device.default_output_config().map_err(io::Error::other)?;
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config = supported.config();

    // O callback do cpal roda em outra thread; as amostras chegam por um canal.
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(64);
    let on_error = |e| eprintln!("áudio: {e}");
    let mono = move |data: &[f32]| -> Vec<f32> {
        data.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => {
            let tx = tx.clone();
            device.build_input_stream(config, move |data: &[f32], _: &_| { let _ = tx.try_send(mono(data)); }, on_error, None)
        }
        SampleFormat::I16 => {
            let tx = tx.clone();
            device.build_input_stream(
                config,
                move |data: &[i16], _: &_| {
                    let floats: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    let _ = tx.try_send(mono(&floats));
                },
                on_error,
                None,
            )
        }
        other => return Err(io::Error::other(format!("formato de áudio não suportado: {other}"))),
    }
    .map_err(io::Error::other)?;
    stream.play().map_err(io::Error::other)?;

    while wanted.load(Ordering::Relaxed) {
        // Sem som tocando, o WASAPI pode não entregar nada; o timeout mantém
        // o loop respondendo a `wanted`.
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(block) => on_samples(&block, rate),
            Err(_) => on_samples(&vec![0.0; rate as usize / 5], rate),
        }
    }
    Ok(())
}
