use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Host, Sample, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{bounded, Receiver};

use crate::encoder::{CaptureFrame, FRAME_SAMPLES, SAMPLE_RATE};

const CHANNEL: &str = "audio_capture";

fn frame_size(rate: u32) -> usize {
    (rate / 50) as usize
}

fn on_stream_error(err: cpal::Error) {
    eprintln!("{CHANNEL}: stream error: {err}");
}

fn pick_loopback_config(
    device: &cpal::Device,
) -> Result<(StreamConfig, SampleFormat, u32), String> {
    let default_supported = device
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
    let default_cfg = default_supported.config();
    let supported = device
        .supported_output_configs()
        .map_err(|e| format!("supported_output_configs: {e}"))?;
    for range in supported {
        if range.contains_rate(SAMPLE_RATE) && range.sample_format() == SampleFormat::F32 {
            return Ok((
                range.with_sample_rate(SAMPLE_RATE).config(),
                SampleFormat::F32,
                SAMPLE_RATE,
            ));
        }
    }
    Ok((
        default_cfg,
        default_supported.sample_format(),
        default_supported.sample_rate(),
    ))
}

pub fn start_capture(
    host: &Host,
    capture_drops: Arc<AtomicU64>,
) -> Result<(Stream, Receiver<CaptureFrame>), String> {
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device for loopback".to_string())?;

    let (cfg, sample_format, rate) = pick_loopback_config(&device)?;
    let frame_len = if rate == SAMPLE_RATE {
        FRAME_SAMPLES
    } else {
        frame_size(rate)
    };

    let (tx, rx) = bounded::<CaptureFrame>(16);
    let drops = Arc::clone(&capture_drops);

    macro_rules! build {
        ($t:ty) => {{
            let tx = tx.clone();
            let drops = Arc::clone(&drops);
            let mut carry: Vec<f32> = Vec::with_capacity(frame_len);
            device.build_input_stream::<$t, _, _>(
                cfg,
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let mut i = 0usize;
                    while i < data.len() {
                        let take = (frame_len.saturating_sub(carry.len())).min(data.len() - i);
                        if take == 0 {
                            break;
                        }
                        for s in &data[i..i + take] {
                            carry.push(s.to_sample::<f32>());
                        }
                        i += take;
                        while carry.len() >= frame_len {
                            let frame: Vec<f32> = carry.drain(..frame_len).collect();
                            if tx
                                .try_send(CaptureFrame {
                                    rate,
                                    samples: frame,
                                })
                                .is_err()
                            {
                                drops.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                },
                on_stream_error,
                None,
            )
        }};
    }

    let stream = match sample_format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::U16 => build!(u16),
        _ => return Err("unsupported sample format".into()),
    }
    .map_err(|e| format!("build_input_stream: {e}"))?;

    stream.play().map_err(|e| format!("play: {e}"))?;

    Ok((stream, rx))
}
