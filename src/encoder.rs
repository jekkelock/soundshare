use std::sync::atomic::{AtomicU32, Ordering};

use opus::{Application, Bitrate, Channels, Decoder, Encoder, Error as OpusError};

pub const SAMPLE_RATE: u32 = 48000;
pub const FRAME_SAMPLES: usize = 960; // 20 ms @ 48 kHz
pub const CHANNELS: usize = 1;
pub const BITRATE_KBPS: i32 = 32;
pub const FRAME_MS: u32 = 20;

pub const MAGIC: [u8; 2] = [0x53, 0x48]; // 'S', 'H'
pub const VERSION: u8 = 1;
pub const SSRC: u32 = 0x53_48_41_72; // 'SHAr' — application-wide static SSRC for discovery
pub const UDP_PORT: u16 = 18090;

pub mod flags {
    pub const STEREO: u8 = 0x01;
    pub const FEC: u8 = 0x02;
}

pub struct CaptureFrame {
    pub rate: u32,
    pub samples: Vec<f32>,
}

const HEADER_LEN: usize = 18;
const OPUS_PAYLOAD_MAX: usize = 1272;

#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub ts_ms: u32,
    pub seq: u32,
    pub ssrc: u32,
    pub stereo: bool,
    pub fec: bool,
    pub payload: Vec<u8>,
}

impl AudioPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        let mut f = 0u8;
        if self.stereo {
            f |= flags::STEREO;
        }
        if self.fec {
            f |= flags::FEC;
        }
        out.push(f);
        out.extend_from_slice(&self.ts_ms.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.ssrc.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN || buf[0] != MAGIC[0] || buf[1] != MAGIC[1] {
            return None;
        }
        if buf[2] != VERSION {
            return None;
        }
        let flag = buf[3];
        let ts_ms = u32::from_le_bytes(buf[4..8].try_into().ok()?);
        let seq = u32::from_le_bytes(buf[8..12].try_into().ok()?);
        let ssrc = u32::from_le_bytes(buf[12..16].try_into().ok()?);
        let opus_len = u16::from_le_bytes(buf[16..18].try_into().ok()?) as usize;
        let start = HEADER_LEN;
        if buf.len() < start + opus_len {
            return None;
        }
        let payload = buf[start..start + opus_len].to_vec();
        Some(AudioPacket {
            ts_ms,
            seq,
            ssrc,
            stereo: flag & flags::STEREO != 0,
            fec: flag & flags::FEC != 0,
            payload,
        })
    }
}

pub struct Codec {
    encoder: Encoder,
    decoder: Decoder,
    ssrc: u32,
    seq_counter: AtomicU32,
}

impl Codec {
    pub fn new(ssrc: u32) -> Result<Self, OpusError> {
        let channels = if CHANNELS == 1 {
            Channels::Mono
        } else {
            Channels::Stereo
        };
        let mut encoder = Encoder::new(SAMPLE_RATE, channels, Application::Audio)?;
        encoder.set_application(Application::Audio)?;
        encoder.set_bitrate(Bitrate::Bits(BITRATE_KBPS * 1000))?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(5)?;

        let mut decoder = Decoder::new(SAMPLE_RATE, channels)?;
        decoder.set_gain(0)?;
        Ok(Self {
            encoder,
            decoder,
            ssrc,
            seq_counter: AtomicU32::new(0),
        })
    }

    pub fn encode_frame(&mut self, pcm: &[f32], ts_ms: u32) -> Result<AudioPacket, OpusError> {
        let payload = self.encoder.encode_vec_float(pcm, OPUS_PAYLOAD_MAX)?;
        let seq = self.seq_counter.fetch_add(1, Ordering::Relaxed);
        Ok(AudioPacket {
            ts_ms,
            seq,
            ssrc: self.ssrc,
            stereo: CHANNELS > 1,
            fec: true,
            payload,
        })
    }

    pub fn decode_frame(&mut self, pkt: &AudioPacket) -> Result<Vec<f32>, OpusError> {
        let out_len = FRAME_SAMPLES * CHANNELS;
        let mut out = vec![0.0f32; out_len];
        let decoded = self.decoder.decode_float(&pkt.payload, &mut out, false)?;
        if decoded != FRAME_SAMPLES {
            out.truncate(decoded.min(out_len));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_round_trip() {
        let pkt = AudioPacket {
            ts_ms: 12345,
            seq: 7,
            ssrc: 0xABCD,
            stereo: true,
            fec: false,
            payload: vec![0x01, 0x02, 0x03, 0x04, 0x05],
        };
        let encoded = pkt.encode();
        assert!(encoded.len() >= HEADER_LEN);
        assert_eq!(&encoded[0..2], &MAGIC);
        assert_eq!(encoded[2], VERSION);

        let decoded = AudioPacket::decode(&encoded).expect("decode failed");
        assert_eq!(decoded.ts_ms, 12345);
        assert_eq!(decoded.seq, 7);
        assert_eq!(decoded.ssrc, 0xABCD);
        assert!(decoded.stereo);
        assert!(!decoded.fec);
        assert_eq!(decoded.payload, vec![0x01, 0x02, 0x03, 0x04, 0x05]);
    }

    #[test]
    fn packet_decode_rejects_short() {
        assert!(AudioPacket::decode(&[0x53]).is_none());
    }

    #[test]
    fn packet_decode_rejects_wrong_magic() {
        let mut buf = vec![0u8; HEADER_LEN + 4];
        buf[0] = 0x00;
        assert!(AudioPacket::decode(&buf).is_none());
    }

    #[test]
    fn packet_decode_rejects_wrong_version() {
        let mut buf = vec![0u8; HEADER_LEN + 4];
        buf[0] = MAGIC[0];
        buf[1] = MAGIC[1];
        buf[2] = 99;
        assert!(AudioPacket::decode(&buf).is_none());
    }
}
