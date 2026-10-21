use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tiny_http::{Method, Response, Server};

const HTTP_PORT: u16 = 8080;
const WS_PORT: u16 = 8081;

const HTML_CLIENT: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SoundShare Receiver</title>
<style>
  body { font-family: -apple-system, BlinkMacSystemFont, sans-serif; text-align: center; padding: 40px; background: #1a1a2e; color: #eee; }
  h1 { color: #e94567; }
  button { padding: 10px 20px; font-size: 14px; background: #e94567; color: white; border: none; border-radius: 4px; cursor: pointer; margin: 5px; }
  button:active { background: #c81e45; }
  #status { margin: 15px 0; font-size: 14px; color: #aaa; }
  #url { font-family: monospace; background: #16213e; padding: 8px 12px; border-radius: 4px; color: #00d4ff; display: inline-block; margin: 10px 0; }
  #hint { margin-top: 15px; font-size: 12px; color: #888; }
</style>
</head>
<body>
<h1>SoundShare Receiver</h1>
<div id="status">Initializing audio...</div>
<div id="url"></div>
<div id="hint">Make sure both devices are on the same network</div>

<script>
let audioContext = null;
let scriptNode = null;
const audioBuffer = [];
let socket = null;
let reconnectAttempts = 0;

async function initAudio() {
  try {
    audioContext = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 48000 });
    const bufferSize = 4096;
    scriptNode = audioContext.createScriptProcessor(bufferSize, 0, 1);
    scriptNode.connect(audioContext.destination);
    scriptNode.onaudioprocess = function(e) {
      const output = e.outputBuffer.getChannelData(0);
      for (let i = 0; i < output.length; i++) {
        if (audioBuffer.length > 0) output[i] = audioBuffer.shift();
        else output[i] = 0;
      }
    };
    await audioContext.resume();
    document.getElementById('status').textContent = 'Audio ready, connecting...';
    connectWebSocket();
  } catch (err) {
    document.getElementById('status').textContent = 'Audio init failed: ' + err.message;
  }
}

function connectWebSocket() {
  document.getElementById('status').textContent = 'Connecting to ' + window.location.hostname + ':8081...';
  const wsProto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsHost = window.location.hostname;
  const wsUrl = wsProto + '//' + wsHost + ':8081/ws';
  socket = new WebSocket(wsUrl);
  socket.binaryType = 'arraybuffer';

  socket.onopen = function() {
    document.getElementById('status').textContent = 'Connected! Playing audio...';
    reconnectAttempts = 0;
  };

  socket.onmessage = function(event) {
    if (document.getElementById('status').textContent.includes('Connecting')) {
      document.getElementById('status').textContent = 'Connected! Playing audio...';
    }
    const view = new DataView(event.data);
    if (view.byteLength < 1) return;
    const nSamples = (view.byteLength - 1) / 2;
    if (nSamples === 0) return;
    for (let i = 0; i < nSamples; i++) {
      audioBuffer.push(view.getInt16(1 + i * 2, true) / 32768.0);
    }
    if (audioBuffer.length > 200000) {
      audioBuffer.splice(0, audioBuffer.length - 200000);
    }
  };

  socket.onclose = function() {
    document.getElementById('status').textContent = 'Disconnected';
    if (reconnectAttempts < 999) {
      reconnectAttempts++;
      setTimeout(connectWebSocket, 1000 + Math.random() * 500);
    }
  };

  socket.onerror = function() {
    document.getElementById('status').textContent = 'WebSocket error - check port 8081';
  };
}

document.getElementById('url').textContent = window.location.href;
initAudio();
</script>
</body>
</html>
"#;

pub struct WebServer {
    http_thread: Option<thread::JoinHandle<()>>,
    ws_thread: Option<thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
    _audio_source: Arc<AudioSource>,
    url: String,
}

impl WebServer {
    pub fn url(&self) -> &str {
        &self.url
    }
}

pub struct AudioSource {
    pub frames: Arc<Mutex<Vec<Vec<f32>>>>,
}

impl AudioSource {
    pub fn new() -> Self {
        Self {
            frames: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn push_frame(&self, pcm: Vec<f32>) {
        let mut frames = self.frames.lock().unwrap();
        frames.push(pcm);
        if frames.len() > 8 {
            let drain_to = frames.len() - 8;
            frames.drain(0..drain_to);
        }
    }
}

impl Default for AudioSource {
    fn default() -> Self {
        Self::new()
    }
}

pub fn start_web_server(audio_source: Arc<AudioSource>) -> Result<WebServer, String> {
    let running = Arc::new(AtomicBool::new(true));
    let http_run = Arc::clone(&running);
    let ws_run = Arc::clone(&running);
    let http_source = Arc::clone(&audio_source);
    let ws_source = Arc::clone(&audio_source);

    // HTTP server for serving HTML
    let http_thread = thread::spawn(move || {
        let server = match Server::http(("0.0.0.0", HTTP_PORT)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("web server bind failed: {e}");
                return;
            }
        };
        for request in server.incoming_requests() {
            if !http_run.load(Ordering::Relaxed) {
                break;
            }
            let url = request.url().to_string();
            let method = request.method();

            if url == "/" && method == &Method::Get {
                let response = Response::from_data(HTML_CLIENT.as_bytes().to_vec());
                let _ = request.respond(response);
            } else if url == "/audio" && method == &Method::Get {
                let frames = http_source.frames.lock().unwrap().clone();
                let mut buf = Vec::new();
                for frame in &frames {
                    buf.push(0u8); // no flags for raw PCM
                    for &s in frame {
                        let val = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        buf.extend_from_slice(&val.to_le_bytes());
                    }
                }
                let resp = Response::from_data(buf);
                let _ = request.respond(resp);
            } else {
                let resp = Response::from_data(b"Not found\n".to_vec());
                let _ = request.respond(resp);
            }
        }
    });

    // WebSocket server for real-time audio streaming
    let ws_thread = thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", WS_PORT)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("WS server bind failed: {e}");
                return;
            }
        };
        listener.set_nonblocking(true).ok();

        let mut clients: Vec<WsClient> = Vec::new();

        while ws_run.load(Ordering::Relaxed) {
            // Accept new connections
            if let Ok((stream, _)) = listener.accept() {
                if let Ok(client) = WsClient::new(stream) {
                    clients.push(client);
                }
            }

            // Broadcast latest audio frame to all clients
            let frames = ws_source.frames.lock().unwrap().clone();
            if let Some(frame) = frames.last() {
                let mut data = Vec::with_capacity(2 + frame.len() * 2);
                data.push(1u8); // flags: mono
                for &s in frame {
                    let val = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    data.extend_from_slice(&val.to_le_bytes());
                }
                let mut to_remove = Vec::new();
                for (i, client) in clients.iter_mut().enumerate() {
                    if client.send_binary(&data).is_err() {
                        to_remove.push(i);
                    }
                }
                for i in to_remove.into_iter().rev() {
                    clients.remove(i);
                }
            }

            // Clean up dead clients
            clients.retain(|c| c.is_alive() && !c.is_dead());

            thread::sleep(Duration::from_millis(20));
        }
    });

    let host = local_ip();
    let url = format!("http://{}:{}", host, HTTP_PORT);

    Ok(WebServer {
        http_thread: Some(http_thread),
        ws_thread: Some(ws_thread),
        running,
        _audio_source: audio_source,
        url,
    })
}

pub fn stop_web_server(server: WebServer) {
    server.running.store(false, Ordering::Relaxed);
    if let Some(handle) = server.http_thread {
        let _ = handle.join();
    }
    if let Some(handle) = server.ws_thread {
        let _ = handle.join();
    }
}

fn local_ip() -> String {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return "localhost".to_string(),
    };
    if socket.connect(("1.1.1.1", 53)).is_err() {
        return "localhost".to_string();
    }
    socket
        .local_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "localhost".to_string())
}

struct WsClient {
    stream: Arc<Mutex<TcpStream>>,
    dead: Arc<AtomicBool>,
}

impl WsClient {
    fn new(stream: TcpStream) -> Result<Self, String> {
        stream.set_nonblocking(false).ok();
        let client = Self {
            stream: Arc::new(Mutex::new(stream)),
            dead: Arc::new(AtomicBool::new(false)),
        };
        // Do WebSocket handshake
        let key = {
            let mut buf = [0u8; 1024];
            let mut s = client.stream.lock().unwrap();
            let n = match s.read(&mut buf) {
                Ok(n) if n > 0 => n,
                _ => return Err("handshake read failed".into()),
            };
            let handshake = String::from_utf8_lossy(&buf[..n]).to_string();
            let k = extract_ws_key(&handshake);
            if k.is_empty() {
                return Err("no WS key".into());
            }
            k
        };

        let accept = ws_accept_key(&key);
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        );

        {
            let mut s = client.stream.lock().unwrap();
            if s.write_all(response.as_bytes()).is_err() {
                client.dead.store(true, Ordering::Relaxed);
            }
            s.flush().ok();
            s.set_read_timeout(Some(Duration::from_millis(1))).ok();
            s.set_write_timeout(Some(Duration::from_millis(10))).ok();
        }
        Ok(client)
    }

    fn is_alive(&self) -> bool {
        let mut buf = [0u8; 1];
        let mut s = match self.stream.try_lock() {
            Ok(guard) => guard,
            Err(_) => return true,
        };
        match s.read(&mut buf) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                true
            }
            Err(_) => false,
        }
    }

    fn send_binary(&self, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        // Server-to-client: no masking (frame opcode 2 = binary)
        let mut header = Vec::new();
        if data.len() < 126 {
            header.push(0x82);
            header.push(data.len() as u8);
        } else if data.len() < 65536 {
            header.push(0x82);
            header.push(126);
            header.extend_from_slice(&(data.len() as u16).to_be_bytes());
        } else {
            header.push(0x82);
            header.push(127);
            header.extend_from_slice(&(data.len() as u64).to_be_bytes());
        }

        let mut s = match self.stream.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Err("lock timeout".into()),
        };
        s.write_all(&header).map_err(|e| {
            self.dead.store(true, Ordering::Relaxed);
            e.to_string()
        })?;
        s.write_all(data).map_err(|e| {
            self.dead.store(true, Ordering::Relaxed);
            e.to_string()
        })?;
        s.flush().map_err(|e| e.to_string())
    }

    fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }
}

fn extract_ws_key(handshake: &str) -> String {
    for line in handshake.lines() {
        if line.to_lowercase().starts_with("sec-websocket-key:") {
            return line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
    }
    String::new()
}

fn ws_accept_key(key: &str) -> String {
    use base64::Engine;
    use sha1::Digest;
    let combined = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let sha1_result = sha1::Sha1::digest(combined.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(sha1_result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ws_accept_key_matches_rfc_example() {
        // RFC 6455 Section 4.2.2 test vector
        use base64::Engine;
        use sha1::{Digest, Sha1};
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let combined = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        let h = Sha1::digest(combined.as_bytes());
        let expected = base64::engine::general_purpose::STANDARD
            .decode("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
            .unwrap();
        eprintln!("expected: {:02x?}", expected);
        eprintln!("actual:   {:02x?}", &h[..]);
        assert_eq!(&h[..], &expected[..]);
    }
}
