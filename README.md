# Camera & Microphone Controller (`cam-mic`)

> **High-performance, low-latency Rust CLI tool** for Linux to manage virtual cameras and microphones, inject media feeds, toggle devices on-the-fly (mute/unmute, blank screen), and ensure microsecond-level Audio/Video synchronization.

---

## 📑 Table of Contents

1. [Features & Capabilities](#-features--capabilities)
2. [Prerequisites & System Requirements](#-prerequisites--system-requirements)
3. [Installation & Build](#-installation--build)
4. [Quick Start](#-quick-start)
5. [CLI Commands Reference](#-cli-commands-reference)
   - [Global Options](#global-options)
   - [`setup`](#1-setup)
   - [`sync-feed`](#2-sync-feed)
   - [`mic`](#3-mic)
   - [`camera`](#4-camera)
   - [`test`](#5-test)
   - [`destroy` / `clean`](#6-destroy--clean)
6. [Interactive Controls & Hotkeys](#-interactive-controls--hotkeys)
7. [IPC & Background Control Architecture](#-ipc--background-control-architecture)
8. [Machine-Readable JSON Output](#-machine-readable-json-output)
9. [Troubleshooting & Verification](#-troubleshooting--verification)

---

## ⚡ Features & Capabilities

- **Native V4L2 & PulseAudio/PipeWire Integration**: Recognized as physical hardware devices by Chrome, Firefox, Zoom, Google Meet, Teams, and WebRTC applications.
- **Hardware-Clock Audio/Video Synchronization**: High-precision monotonic clock guarantees zero AV drift and microsecond-level sync.
- **Real-time Device State Toggling (IPC)**: Dynamically mute/unmute microphones or blank the video feed into black frames without dropping the stream or disconnecting the camera.
- **Interactive Hotkeys (Single-key raw mode)**: Instant keyboard controls for seeking, volume, pause, track navigation, and device toggling.
- **Playlist & Multi-file Feeding**: Sequentially streams multiple video or audio files, supports playlist files, and continuous looping.
- **Clean State Recovery**: Complete teardown command restores the system's default audio sinks and video devices without residue.

---

## 🛠 Prerequisites & System Requirements

Designed and optimized for Linux (Ubuntu/Debian, Fedora, Arch Linux, etc.).

### 1. Required System Packages

```bash
# Ubuntu / Debian
sudo apt update
sudo apt install -y ffmpeg v4l2loopback-dkms pulseaudio-utils v4l-utils
```

### 2. V4L2 Loopback Kernel Module (for Virtual Camera)

To enable the virtual camera device (`/dev/video10`), load the kernel module:

```bash
sudo modprobe v4l2loopback exclusive_caps=1 card_label="VirtualCamera" video_nr=10
```

*(If the kernel module is not present, the tool automatically falls back to a raw FIFO sink)*

### 3. Rust Toolchain

Make sure Rust (2024 edition) and Cargo are installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## 📦 Installation & Build

Clone the repository and compile the project:

```bash
git clone https://github.com/your-username/camera-mic-controller.git
cd camera-mic-controller

# Build optimized release binary
cargo build --release

# (Optional) Install system-wide
sudo cp target/release/camera-mic-controller /usr/local/bin/cam-mic
```

*(You can run the tool via `cargo run -- <command>` or via the installed binary `cam-mic <command>`)*

---

## 🚀 Quick Start

### Step 1: Provision Virtual Devices

```bash
cargo run -- setup
```

This registers the virtual microphone (`Virtual_Mic`) and verifies the virtual camera device (`/dev/video10`).

### Step 2: Stream Synchronized Video & Audio

```bash
cargo run -- sync-feed sample.mp4 --loop
```

Open a browser and verify your feeds on <https://webcamtests.com> and <https://mictests.com>!

---

## 📖 CLI Commands Reference

### Global Options

- `--json`: Output command responses in structured, machine-readable JSON.
- `-h, --help`: Display help information for the tool or subcommands.
- `-V, --version`: Display the version.

---

### 1. `setup`

Checks the operating system environment and provisions virtual hardware devices:

- Sets up PulseAudio/PipeWire virtual mic sink (`virtual_mic_sink`) and remapped source (`Virtual_Mic`).
- Sets the virtual microphone as the default system recording device.
- Verifies the availability of the `/dev/video10` V4L2 device.

```bash
cargo run -- setup
```

---

### 2. `sync-feed`

Streams a video file's video stream into the virtual camera and its audio stream into the virtual microphone with synchronized timestamps.

```bash
cargo run -- sync-feed [OPTIONS] [FILES]...
```

#### Arguments

- `[FILES]...`: Path(s) to input media file(s) (`.mp4`, `.mkv`, `.avi`, `.mov`, etc.).

#### Options

| Option | Default | Description |
| --- | --- | --- |
| `--playlist <PATH>` | `None` | Path to a text file containing media paths (one per line) |
| `-l, --loop` | `false` | Loop playback continuously |
| `-s, --seek <SECONDS>` | `None` | Initial start offset in seconds (e.g. `-s 15.0`) |
| `-v, --volume <0-150>` | `100` | Initial audio volume percentage |
| `--device <PATH>` | `/dev/video10` | Target V4L2 loopback device or FIFO sink |
| `--width <WIDTH>` | `640` | Output frame width |
| `--height <HEIGHT>` | `480` | Output frame height |
| `--fps <FPS>` | `30` | Output video framerate |
| `--sample-rate <HZ>` | `48000` | Target audio sample rate |

#### Examples

```bash
# Loop a single video indefinitely:
cargo run -- sync-feed presentation.mp4 --loop

# Stream at 1080p 60 FPS:
cargo run -- sync-feed video.mp4 --width 1920 --height 1080 --fps 60

# Seek 45 seconds ahead and set initial volume to 80%:
cargo run -- sync-feed lecture.mp4 -s 45 -v 80

# Stream using a playlist file:
cargo run -- sync-feed --playlist videos.txt --loop
```

---

### 3. `mic`

Manage the virtual microphone state, audio-only feeds, and device lifecycle.

```bash
cargo run -- mic <COMMAND>
```

#### Subcommands

#### a) `mic on`

Unmutes the virtual microphone.

```bash
cargo run -- mic on
```

#### b) `mic off`

Mutes the virtual microphone.

```bash
cargo run -- mic off
```

#### c) `mic status`

Displays the current virtual microphone status (Sink/Source names, Mute state, Active stream).

```bash
cargo run -- mic status
# JSON mode:
cargo run -- --json mic status
```

#### d) `mic feed`

Injects an audio track directly into the virtual microphone.

```bash
cargo run -- mic feed [FILES]... [OPTIONS]
```

- `--playlist <PATH>`: Text file containing list of audio paths.
- `-l, --loop`: Loop playback continuously.
- `-s, --seek <SECONDS>`: Start position in seconds.
- `-v, --volume <0-150>`: Initial volume percentage (Default: 100).

```bash
# Example:
cargo run -- mic feed podcast.mp3 --loop --volume 90
```

#### e) `mic teardown`

Unloads virtual audio sinks and sources, restoring your original default recording device.

```bash
cargo run -- mic teardown
```

---

### 4. `camera`

Manage the virtual camera state, video-only feeds, and device lifecycle.

```bash
cargo run -- camera <COMMAND>
```

#### Subcommands

#### a) `camera on`

Enables live video frame forwarding.

```bash
cargo run -- camera on
```

#### b) `camera off`

Injects blank black frames into the camera feed without disconnecting the webcam stream.

```bash
cargo run -- camera off
```

#### c) `camera status`

Displays the virtual camera hardware device path, format, streaming state, and resolution.

```bash
cargo run -- camera status
# JSON mode:
cargo run -- --json camera status
```

#### d) `camera feed`

Feeds video files directly to the virtual camera device (video only).

```bash
cargo run -- camera feed input.mp4 --loop --width 1280 --height 720 --fps 30
```

#### e) `camera teardown`

Cleans up virtual camera temporary frame buffers and FIFO sinks.

```bash
cargo run -- camera teardown
```

---

### 5. `test`

Executes an end-to-end automated verification suite testing the hardware pipeline, audio playback, video decoding, latency, and A/V clock drift.

```bash
cargo run -- test
# In JSON format:
cargo run -- --json test
```

---

### 6. `destroy` / `clean`

Tears down all virtual camera buffers, IPC sockets, and virtual audio devices, resetting the operating system to its original state.

```bash
cargo run -- destroy
# or alias:
cargo run -- clean
```

---

## 🎮 Interactive Controls & Hotkeys

While streaming in `sync-feed`, `camera feed`, or `mic feed`, you can interact with the stream using single-key keyboard shortcuts:

| Key | Action |
| :---: | --- |
| <kbd>Space</kbd> / <kbd>k</kbd> | Toggle **Pause / Resume** |
| <kbd>m</kbd> | Toggle **Mic Mute / Unmute** |
| <kbd>c</kbd> | Toggle **Camera Video / Blank Screen** |
| <kbd>n</kbd> | Skip to **Next Track** |
| <kbd>p</kbd> | Return to **Previous Track** |
| <kbd>f</kbd> / <kbd>&gt;</kbd> | Fast forward **+10 seconds** |
| <kbd>b</kbd> / <kbd>&lt;</kbd> | Rewind **-10 seconds** |
| <kbd>+</kbd> / <kbd>u</kbd> | Increase volume by **+5%** |
| <kbd>-</kbd> / <kbd>d</kbd> | Decrease volume by **-5%** |
| <kbd>l</kbd> | Toggle **Loop Mode** (ON/OFF) |
| <kbd>q</kbd> / <kbd>Ctrl+C</kbd> | Stop playback and restore terminal mode |

---

## 🔌 IPC & Background Control Architecture

You can control an ongoing stream from a separate terminal window, background script, or automated process via UNIX domain socket IPC (`/tmp/cam_mic_controller.sock`):

```bash
# Terminal 1: Active streaming session
cargo run -- sync-feed webinar.mp4 --loop

# Terminal 2: Mute virtual microphone in real-time
cargo run -- mic off

# Terminal 2: Blank camera feed to black frames
cargo run -- camera off

# Terminal 2: Restore live video and audio
cargo run -- mic on
cargo run -- camera on
```

---

## 📊 Machine-Readable JSON Output

All commands support the `--json` flag for scripting, dashboard integration, or automated workflows:

```bash
$ cargo run -- --json mic status
{
  "exists": true,
  "source_name": "Virtual_Mic",
  "sink_name": "virtual_mic_sink",
  "is_muted": false
}

$ cargo run -- --json camera status
{
  "device_path": "/dev/video10",
  "is_v4l2": true,
  "exists": true,
  "active_streaming": true,
  "is_on": true,
  "default_width": 640,
  "default_height": 480,
  "default_fps": 30
}
```

---

## 🔍 Troubleshooting & Verification

1. **Virtual Camera not appearing in web browser / meeting app?**
   - Verify that the `v4l2loopback` kernel module is loaded:

     ```bash
     ls -l /dev/video10
     ```

   - If missing, load it:

     ```bash
     sudo modprobe v4l2loopback exclusive_caps=1 card_label="VirtualCamera" video_nr=10
     ```

   - Restart the browser and check <https://webcamtests.com>.

2. **No audio heard from the Virtual Microphone?**
   - Ensure the recording input device in your system settings is set to `Virtual_Mic`.
   - Check if the mic is muted:

     ```bash
     cargo run -- mic status
     ```

   - Unmute if necessary:

     ```bash
     cargo run -- mic on
     ```

3. **Terminal formatting distorted after process interruption?**
   - The CLI includes RAII terminal guards and signal hooks to restore your terminal automatically. If needed, you can manually reset your terminal:

     ```bash
     reset
     ```
