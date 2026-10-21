use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Host, Stream};

use crate::web_server::{AudioSource, WebServer};

#[derive(Default, Clone)]
pub struct Stats {
    pub running: bool,
    pub packets_sent: u64,
    pub drops: u64,
    pub capture_drops: u64,
    pub rtt_ms: f32,
    pub encode_cpu_pct: f32,
    pub web_clients: usize,
    pub last_error: Option<String>,
}

pub struct StatsHandle(Arc<Mutex<Stats>>);

impl Clone for StatsHandle {
    fn clone(&self) -> Self {
        StatsHandle(Arc::clone(&self.0))
    }
}

impl Default for StatsHandle {
    fn default() -> Self {
        StatsHandle(Arc::new(Mutex::new(Stats::default())))
    }
}

impl StatsHandle {
    pub fn snapshot(&self) -> Stats {
        self.0.lock().unwrap().clone()
    }
    fn with<F: FnOnce(&mut Stats)>(&self, f: F) {
        if let Ok(mut s) = self.0.lock() {
            f(&mut s);
        }
    }
    pub fn set_running(&self, v: bool) {
        self.with(|s| s.running = v);
    }
    pub fn set_error(&self, e: impl Into<String>) {
        self.with(|s| {
            s.last_error = Some(e.into());
        });
    }
    pub fn inc_sent(&self, n: u64) {
        self.with(|s| s.packets_sent += n);
    }
    pub fn inc_drops(&self, n: u64) {
        self.with(|s| s.drops += n);
    }
    pub fn add_capture_drops(&self, n: u64) {
        self.with(|s| s.capture_drops += n);
    }
    pub fn set_rtt(&self, v: f32) {
        self.with(|s| s.rtt_ms = v);
    }
    pub fn set_encode_cpu(&self, v: f32) {
        self.with(|s| s.encode_cpu_pct = v);
    }
    pub fn reset(&self) {
        self.with(|s| {
            *s = Stats {
                running: true,
                ..Default::default()
            };
        });
    }
}

pub struct EngineHandle(pub Arc<Mutex<Engine>>);

impl Clone for EngineHandle {
    fn clone(&self) -> Self {
        EngineHandle(Arc::clone(&self.0))
    }
}

pub struct Engine {
    host: Host,
    running: Arc<AtomicBool>,
    stats: StatsHandle,
    capture_stream: Option<Stream>,
    worker_handles: Vec<JoinHandle<()>>,
    audio_source: Arc<AudioSource>,
    web_server: Option<WebServer>,
}

impl Engine {
    pub fn new() -> Self {
        let host = cpal::default_host();
        Self {
            host,
            running: Arc::new(AtomicBool::new(false)),
            stats: StatsHandle::default(),
            capture_stream: None,
            worker_handles: Vec::new(),
            audio_source: Arc::new(AudioSource::new()),
            web_server: None,
        }
    }

    pub fn snapshot(&self) -> Stats {
        self.stats.snapshot()
    }

    pub fn set_error(&mut self, e: impl Into<String>) {
        self.stats.set_error(e);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn web_url(&self) -> Option<String> {
        self.web_server.as_ref().map(|w| w.url().to_string())
    }

    pub fn device_names(&self, kind: DeviceKind) -> Vec<String> {
        let iter = match kind {
            DeviceKind::Output => self.host.output_devices(),
            DeviceKind::Input => self.host.input_devices(),
        };
        match iter {
            Ok(devs) => devs
                .filter_map(|d| d.id().map(|id| id.to_string()).ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn start(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            self.stats.set_error("already running");
            return;
        }
        let running = Arc::clone(&self.running);
        self.running.store(true, Ordering::SeqCst);
        self.stats.reset();

        // Start web server
        let audio_source = Arc::clone(&self.audio_source);
        match crate::web_server::start_web_server(audio_source) {
            Ok(ws) => {
                self.web_server = Some(ws);
            }
            Err(e) => {
                self.stats.set_error(format!("web server: {e}"));
                self.running.store(false, Ordering::SeqCst);
                self.stats.set_running(false);
                return;
            }
        }

        // Start audio capture
        let drops = Arc::new(AtomicU64::new(0));
        match crate::audio_capture::start_capture(&self.host, Arc::clone(&drops)) {
            Ok((stream, rx)) => {
                self.capture_stream = Some(stream);
                let stats = self.stats.clone();
                let rcpu = Arc::clone(&drops);
                let audio_src = Arc::clone(&self.audio_source);
                self.worker_handles.push(std::thread::spawn(move || {
                    crate::net_sender::run_send_loop(rx, stats, running, rcpu, audio_src);
                }));
            }
            Err(e) => {
                self.stats.set_error(e);
                self.running.store(false, Ordering::SeqCst);
                self.stats.set_running(false);
                if let Some(ws) = self.web_server.take() {
                    crate::web_server::stop_web_server(ws);
                }
            }
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        for h in self.worker_handles.drain(..) {
            let _ = h.join();
        }
        if let Some(ws) = self.web_server.take() {
            crate::web_server::stop_web_server(ws);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.capture_stream = None;
        self.stats.set_running(false);
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

pub enum DeviceKind {
    Input,
    Output,
}

pub fn engine_handle() -> EngineHandle {
    EngineHandle(Arc::new(Mutex::new(Engine::new())))
}
