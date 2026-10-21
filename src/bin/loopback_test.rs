use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use soundshare::encoder::{AudioPacket, Codec, FRAME_MS, FRAME_SAMPLES, SAMPLE_RATE};

const LISTEN: &str = "127.0.0.1:18099";
const FRAMES: usize = 500;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    soundshare::util::init_logging();

    let running = Arc::new(AtomicBool::new(true));
    let sent: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let recv: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let drops: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let oos: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let rtt_sum: Arc<Mutex<u128>> = Arc::new(Mutex::new(0u128));
    let rtt_max: Arc<Mutex<u128>> = Arc::new(Mutex::new(0u128));
    let rtt_n: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let ssrc: u32 = 0xABCD_1234;
    let (net_tx, net_rx) = crossbeam_channel::bounded::<AudioPacket>(64);
    let (play_tx, play_rx) = crossbeam_channel::bounded::<Vec<f32>>(64);
    let _ = play_rx;

    let sender_running = Arc::clone(&running);
    let send_sock = UdpSocket::bind("127.0.0.1:0")?;
    send_sock.set_read_timeout(Some(Duration::from_millis(20)))?;
    let send_target: std::net::SocketAddr = LISTEN.parse()?;
    let sent_c = Arc::clone(&sent);
    let drops_c = Arc::clone(&drops);
    let rtt_sum_c = Arc::clone(&rtt_sum);
    let rtt_max_c = Arc::clone(&rtt_max);
    let rtt_n_c = Arc::clone(&rtt_n);
    let start = Instant::now();

    let sender = thread::spawn(move || {
        let mut codec = Codec::new(ssrc).expect("codec");
        let mut echo = [0u8; 1500];
        for _ in 0..FRAMES {
            if !sender_running.load(Ordering::Relaxed) {
                break;
            }
            let ts_ms = (Instant::now() - start).as_millis() as u32;
            let mut pcm = vec![0.0f32; FRAME_SAMPLES];
            for (j, s) in pcm.iter_mut().enumerate() {
                let t = ts_ms as f32 / 1000.0 + j as f32 / SAMPLE_RATE as f32;
                *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            }
            let pkt = match codec.encode_frame(&pcm, ts_ms) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("encode err {e}");
                    continue;
                }
            };
            let bytes = pkt.encode();
            match send_sock.send_to(&bytes, send_target) {
                Ok(_) => {
                    sent_c.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    drops_c.fetch_add(1, Ordering::Relaxed);
                    eprintln!("send err {e}");
                }
            }
            for _ in 0..8 {
                match send_sock.recv_from(&mut echo) {
                    Ok((n, _)) => {
                        if let Some(p) = AudioPacket::decode(&echo[..n]) {
                            let now_ms = (Instant::now() - start).as_millis();
                            let rtt = now_ms.saturating_sub(p.ts_ms as u128);
                            let mut mx = rtt_max_c.lock().unwrap();
                            if rtt > *mx {
                                *mx = rtt;
                            }
                            *rtt_sum_c.lock().unwrap() += rtt;
                            rtt_n_c.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => break,
                }
            }
            thread::sleep(Duration::from_millis(FRAME_MS as u64));
        }
    });

    let recv_running = Arc::clone(&running);
    let listen_sock = UdpSocket::bind(LISTEN)?;
    listen_sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let recv_c = Arc::clone(&recv);
    let recv_drops = Arc::clone(&drops);
    let net_tx_recv = net_tx.clone();
    let receiver = thread::spawn(move || {
        let mut buf = [0u8; 1500];
        while recv_running.load(Ordering::Relaxed) {
            match listen_sock.recv_from(&mut buf) {
                Ok((n, src)) => {
                    if let Some(pkt) = AudioPacket::decode(&buf[..n]) {
                        if net_tx_recv.try_send(pkt).is_ok() {
                            recv_c.fetch_add(1, Ordering::Relaxed);
                        } else {
                            recv_drops.fetch_add(1, Ordering::Relaxed);
                        }
                        let _ = listen_sock.send_to(&buf[..n], src);
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue
                }
                Err(_) => break,
            }
        }
    });

    let jitter_running = Arc::clone(&running);
    let play_tx_c = play_tx.clone();
    let mut decoded = 0usize;
    let jitter = thread::spawn(move || {
        let mut codec = Codec::new(0).expect("codec");
        while jitter_running.load(Ordering::Relaxed) {
            match net_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(pkt) => match codec.decode_frame(&pkt) {
                    Ok(pcm) => {
                        let _ = play_tx_c.try_send(pcm);
                        decoded += 1;
                    }
                    Err(e) => {
                        eprintln!("decode err {e}");
                    }
                },
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }
        decoded
    });

    sender.join().expect("sender");
    thread::sleep(Duration::from_millis(400));
    running.store(false, Ordering::SeqCst);

    receiver.join().expect("receiver");
    let _ = net_tx.send(AudioPacket {
        ts_ms: 0,
        seq: 0,
        ssrc,
        stereo: false,
        fec: false,
        payload: Vec::new(),
    });
    let decoded = jitter.join().expect("jitter");

    let s = sent.load(Ordering::Relaxed);
    let r = recv.load(Ordering::Relaxed);
    let d = drops.load(Ordering::Relaxed);
    let o = oos.load(Ordering::Relaxed);
    let rtt_avg = {
        let n = rtt_n.load(Ordering::Relaxed);
        if n > 0 {
            *rtt_sum.lock().unwrap() / n as u128
        } else {
            0
        }
    };
    let rttmax = *rtt_max.lock().unwrap();
    let loss = if s > 0 {
        d as f32 / s as f32 * 100.0
    } else {
        0.0
    };

    println!("=== SoundShare loopback_test ===");
    println!("frames_sent    : {s}");
    println!("packets_recv   : {r}");
    println!("dropped        : {d}");
    println!("decode_oos     : {o}");
    println!("loss %%         : {loss:.2}");
    println!("frames_decoded : {decoded}");
    println!("rtt avg        : {rtt_avg} ms");
    println!("rtt max        : {rttmax} ms");
    println!("opus version   : {}", opus::version());
    println!(
        "codec          : 48 kHz mono, {} kbps",
        soundshare::encoder::BITRATE_KBPS
    );
    Ok(())
}
