# SoundShare

Ultra-lightweight, cross-platform whole-system audio loopback streaming over LAN.

A single Rust binary (GPL-3.0) that captures system audio (WASAPI loopback on
Windows, CoreAudio/ScreenCaptureKit on macOS, PipeWire/PulseAudio on Linux),
encodes it with **Opus**, ships it over **UDP**, and plays it back with
sub-60 ms end-to-end latency. One-to-few direct IP streaming with a tiny
`egui` GUI — no WebRTC, no GStreamer, no WebView.

## Build

```sh
cargo build --release
```

## Hard limits

- Binary: < 25 MB
- Idle RAM: < 40 MB
- LAN latency: <= 60 ms target / 80 ms ceiling
- Drop rate: < 1 % under saturated Wi-Fi

## License

GPL-3.0-or-later. Note: GPLv3 is incompatible with App Store distribution.
