use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::encoder::{CaptureFrame, Codec, FRAME_SAMPLES, SAMPLE_RATE, UDP_PORT};
use crate::engine::StatsHandle;
use crate::web_server::AudioSource;

struct Resampler {
    ratio: f64,
    cursor: f64,
    backlog: Vec<f32>,
}

impl Resampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            ratio: in_rate as f64 / out_rate as f64,
            cursor: 0.0,
            backlog: Vec::with_capacity(1024),
        }
    }

    fn convert(&mut self, input: &[f32], out_count: usize) -> Vec<f32> {
        self.backlog.extend_from_slice(input);
        let mut out = Vec::with_capacity(out_count);
        for _ in 0..out_count {
            let i = self.cursor as usize;
            let f = self.cursor - (i as f64);
            if i + 1 < self.backlog.len() {
                let s0 = self.backlog[i];
                let s1 = self.backlog[i + 1];
                out.push(s0 + (s1 - s0) * (f as f32));
            } else if let Some(last) = self.backlog.last() {
                out.push(*last);
            } else {
                out.push(0.0);
            }
            self.cursor += self.ratio;
        }
        let drop = (self.cursor as usize).min(self.backlog.len());
        if drop > 0 {
            self.backlog.drain(..drop);
            self.cursor -= drop as f64;
        }
        out
    }
}

pub fn run_send_loop(
    rx: Receiver<CaptureFrame>,
    stats: StatsHandle,
    running: Arc<AtomicBool>,
    capture_drops: Arc<AtomicU64>,
    audio_source: Arc<AudioSource>,
) {
    let mut codec = match Codec::new(crate::encoder::SSRC) {
        Ok(c) => c,
        Err(e) => {
            stats.set_error(format!("codec: {e}"));
            return;
        }
    };

    let udp_sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => {
            let _ = s.set_broadcast(true);
            s
        }
        Err(e) => {
            stats.set_error(format!("udp bind: {e}"));
            return;
        }
    };
    let udp_target: SocketAddr = format!("255.255.255.255:{}", UDP_PORT)
        .parse()
        .expect("valid broadcast addr");

    let mut resampler: Option<Resampler> = None;
    let start = Instant::now();
    let mut period = start;
    let mut last_drops_check = start;
    let mut last_drops = 0u64;
    let mut encode_cpu_time = 0f32;

    while running.load(Ordering::Relaxed) {
        let frame = match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let work_start = Instant::now();

        let pcm = if frame.rate != SAMPLE_RATE {
            if resampler.is_none() {
                resampler = Some(Resampler::new(frame.rate, SAMPLE_RATE));
            }
            resampler
                .as_mut()
                .unwrap()
                .convert(&frame.samples, FRAME_SAMPLES)
        } else {
            frame.samples
        };

        if let Err(e) = encode_and_send(&pcm, &mut codec, &start, &stats, &audio_source, &udp_sock, &udp_target) {
            stats.set_error(e);
        }

        encode_cpu_time += work_start.elapsed().as_secs_f32();

        let now = Instant::now();
        if now.duration_since(period) > Duration::from_secs(1) {
            let wall = now.duration_since(period).as_secs_f32();
            let cpu_pct = if wall > 0.0 { encode_cpu_time / wall } else { 0.0 };
            stats.set_encode_cpu(cpu_pct.min(1.0));
            encode_cpu_time = 0.0;
            period = now;
        }

        if now.duration_since(last_drops_check) > Duration::from_millis(500) {
            let current = capture_drops.load(Ordering::Relaxed);
            stats.add_capture_drops(current.saturating_sub(last_drops));
            last_drops = current;
            last_drops_check = now;
        }
    }
}

fn encode_and_send(
    pcm: &[f32],
    codec: &mut Codec,
    start: &Instant,
    stats: &StatsHandle,
    audio_source: &Arc<AudioSource>,
    udp_sock: &UdpSocket,
    udp_target: &SocketAddr,
) -> Result<(), String> {
    let ts_ms = (Instant::now() - *start).as_millis() as u32;
    let pkt = codec
        .encode_frame(pcm, ts_ms)
        .map_err(|e| format!("opus encode: {e}"))?;

    let bytes = pkt.encode();
    match udp_sock.send_to(&bytes, udp_target) {
        Ok(_) => stats.inc_sent(1),
        Err(e) => stats.set_error(format!("udp send: {e}")),
    }

    audio_source.push_frame(pcm.to_vec());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_same_rate_passthrough() {
        let mut r = Resampler::new(48000, 48000);
        let input = vec![1.0f32, -1.0, 0.5, -0.5, 2.0];
        let out = r.convert(&input, 4);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[1], -1.0);
        assert_eq!(out[2], 0.5);
        assert_eq!(out[3], -0.5);
    }

    #[test]
    fn resampler_carries_over() {
        // 2:1 downsampling: 4 input → 2 output, then 4 more input → 2 output
        let mut r = Resampler::new(96000, 48000);
        let chunk = vec![1.0f32, 2.0, 3.0, 4.0];
        let out1 = r.convert(&chunk, 2);
        assert_eq!(out1.len(), 2);
        let out2 = r.convert(&chunk, 2);
        assert_eq!(out2.len(), 2);
        // No panic, no infinite growth
        let out3 = r.convert(&chunk, 2);
        assert_eq!(out3.len(), 2);
    }
}
