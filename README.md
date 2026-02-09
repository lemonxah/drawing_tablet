# Drawing Tablet

A high-performance, low-latency drawing tablet solution that turns your Android device into a professional graphics tablet for Linux (Wayland).

## 🚀 Overview

Check out the [Official website](https://lemonxah.github.io/drawing_tablet/)

This project consists of two parts:
1.  **Android Client:** A dedicated app that captures stylus input (pressure, tilt, coordinates) and renders a low-latency video stream of your PC screen.
2.  **Rust Server:** A lightweight Linux daemon that:
    *   Captures the screen using **PipeWire** (via XDG Desktop Portals).
    *   Encodes the video stream using **GStreamer** (Hardware Accelerated VA-API/NVENC).
    *   Emulates a virtual Wacom tablet using **uinput** to inject pen/touch events into the OS.

The result is a Cintiq-like experience on your Android tablet, entirely over WiFi.

## ✨ Features

*   **Millisecond Input Latency:** TCP-based protocol for instant cursor response.
*   **High-Quality Screen Mirroring:** H.264/H.265 hardware encoding.
*   **Full Wacom Emulation:** Supports Pressure, Tilt, and Hover.
*   **Gesture Support:** Pinch-to-zoom, Pan, and Rotate support in apps like Krita/GIMP (Server-side gesture handling).
*   **Wayland Native:** Built for modern Linux desktops (GNOME/KDE/Sway).
*   **Auto-Connect:** Client remembers your server and handles network interruptions gracefully.

---

## 🛠️ Prerequisites

### Linux Server
You need a modern Linux distribution running **Wayland**.

**Build Dependencies (Debian/Ubuntu):**
```bash
sudo apt install \
    build-essential \
    pkg-config \
    libclang-dev \
    libpipewire-0.3-dev \
    libglib2.0-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libgstreamer-plugins-bad1.0-dev \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-vaapi \
    udev
```

**Build Dependencies (Fedora):**
```bash
sudo dnf install \
    clang \
    pipewire-devel \
    glib2-devel \
    gstreamer1-devel \
    gstreamer1-plugins-base-devel \
    gstreamer1-plugins-bad-free-devel
```

**Build Dependencies (Arch Linux):**
```bash
sudo pacman -S base-devel clang pipewire glib2 gstreamer \
    gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly \
    gstreamer-vaapi
```

**Build Dependencies (Gentoo):**
```bash
# Note: Ensure USE flags like 'pipewire' and 'vaapi' are enabled globally or for relevant packages.
sudo emerge --ask \
    dev-util/pkgconf \
    sys-devel/clang \
    media-video/pipewire \
    media-libs/gstreamer \
    media-libs/gst-plugins-base \
    media-libs/gst-plugins-good \
    media-libs/gst-plugins-bad \
    media-libs/gst-plugins-ugly \
    media-plugins/gst-plugins-vaapi
```

**Rust Toolchain:**
Install [Rustup](https://rustup.rs/):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Android Client
*   **Android Studio** (Koala or newer recommended).
*   **Android SDK Platform 34** (Android 14).
*   **NDK** (Side-by-side).

---

## 📦 Compilation & Installation

We use [just](https://github.com/casey/just) as a command runner. Install it (optional but recommended) or copy the commands from the `justfile`.

### 1. Build the Server
```bash
# Debug build
just build

# Release build (Optimized)
just build-release
```

### 2. Set up udev Rules (Important!)
To allow the server to create virtual input devices without `sudo`, you need a udev rule.

Create `/etc/udev/rules.d/99-drawing-tablet.rules`:
```bash
KERNEL=="uinput", GROUP="input", MODE="0660"
```
Then run:
```bash
sudo udevadm control --reload-rules
sudo udevadm trigger
sudo usermod -aG input $USER
```
*Log out and back in for the group change to take effect.*

### 3. Build the Android App
Open the `android/` directory in Android Studio and hit **Run**, or use the CLI:
```bash
just android-deploy
```

---

## 🎮 Usage

### 1. Start the Server
```bash
just run-release
```
*   On first run, a "Screen Share" dialog will appear (handled by XDG Portal). Select the screen you want to mirror.
*   The server listens on **TCP/UDP Port 9867** (TCP for input, UDP for video).

### 2. Start the Client
1.  Open the **Drawing Tablet** app on Android.
2.  The app will automatically scan for the server on your local network.
3.  When your computer appears in the list, **tap it to connect**.
    *   *Optional:* Check **"Remember this server"** to auto-connect in the future.

### 3. Draw!
*   **Pen:** Drawing, pressure, and tilt should work immediately in GIMP/Krita/Blender.
*   **Touch:**
    *   **1 Finger:** Ignored (to prevent accidental drawing).
    *   **2+ Fingers:** Zoom/Pan/Rotate gestures.

---

## 🎨 Application Setup

### Krita Configuration

For the best drawing experience in Krita, you need to adjust the brush smoothing settings:

1. Open Krita and select any brush tool
2. In the **Tool Options** docker (usually on the right side), find the **Brush Smoothing** section at the bottom
3. Change the smoothing mode from **Weighted** to **Basic** or **None**

**Why?** Krita's "Weighted Smoothing" algorithm uses time-based interpolation that can cause line spikes when input events arrive with irregular timing (due to network jitter). The "Basic" smoothing mode works perfectly with this tablet.

| Smoothing Mode | Compatibility |
|:--------------|:--------------|
| None | ✅ Works perfectly |
| Basic | ⚠️ May cause line spikes  |
| Weighted | ✅  Works perfectly |
| Stabilizer | ✅ Works (adds input lag) |

---

## 🏗️ Architecture Deep Dive

### 1. Screen Capture (`crates/dt-capture`)
We bypass the slow X11 capture methods by using **PipeWire** directly.
*   **XDG Desktop Portal:** We request a session via DBus (`ashpd` crate). The user selects a monitor securely.
*   **PipeWire Stream:** We negotiate a raw video buffer (BGRx) format with the compositor. Memory is shared via `MemFd` (Zero-copy where possible) for maximum performance.

### 2. Video Encoding (`crates/dt-encoder`)
Raw pixels are heavy (4K @ 60fps is huge). We pipe the buffer into **GStreamer**.
*   **Pipeline:** `appsrc -> videoconvert -> vaapih264enc -> rtph264pay -> udpsink`
*   **Hardware Acceleration:** We explicitly target `vaapih264enc` (Intel/AMD) or `nvh264enc` (Nvidia) to keep CPU usage low (< 5%).
*   **Network:** The encoded NAL units are packetized into RTP and blasted over UDP. We prioritize **latency over reliability**—dropped frames are better than old frames.

### 3. Input Emulation (`crates/dt-input`)
We use the Linux kernel's `uinput` module to create two virtual devices:
1.  **"Drawing Tablet Pen":** A high-resolution absolute input device. It reports `ABS_X`, `ABS_Y`, `ABS_PRESSURE`, `ABS_TILT_X`, `ABS_TILT_Y`.
2.  **"Drawing Tablet Touch":** A specialized Multitouch device.
    *   **Mode:** It acts as a "Direct" input device but **without** button capabilities (`BTN_TOUCH` removed).
    *   **Why?** This prevents GIMP from treating a single finger as a "Mouse Click" (Selection Tool) while still allowing the OS (libinput) to recognize multi-touch gestures like Pinch-to-Zoom.

---

## 📜 Justfile Commands

| Command | Description |
| :--- | :--- |
| `just build` | Build server (Debug) |
| `just run` | Run server (Debug) |
| `just run-release` | Run server (Release - Recommended) |
| `just android-deploy` | Build & Install APK to connected device |
| `just android-logs` | Tail filtered Android logs |
| `just check-pipewire` | Verify PipeWire status |
| `just check-vaapi` | Verify Hardware Encoder availability |

---

## ❓ Troubleshooting

**Q: Connection fails immediately.**
*   Check your firewall (`sudo ufw allow 9867/tcp` and `sudo ufw allow 9867/udp`).
*   Ensure both devices are on the same 5GHz WiFi network.

**Q: "Failed to create virtual device" error.**
*   You missed the **udev rules** step. Run the commands in the Installation section or run the server with `sudo` (not recommended).

**Q: The screen is black on Android.**
*   Check the server logs. If `vaapi` fails, install `gstreamer1.0-vaapi`.
*   Ensure you selected a screen in the portal dialog.

**Q: GIMP selects instead of zooming.**
*   We filtered single-touch inputs to fix this. Ensure you are running the latest server version.

**Q: Lines have random spikes/glitches in Krita.**
*   This is caused by Krita's "Weighted Smoothing" brush setting. Change it to "Basic" or "None" in the Tool Options docker. See the [Krita Configuration](#krita-configuration) section above.

---

## 📄 License

This project is licensed under the **MIT License** - see the [LICENSE](LICENSE) file for details.

### Third-Party Libraries

This software uses the following libraries at runtime:
- **GStreamer** (LGPL-2.1+) - Multimedia framework
- **PipeWire** (MIT/LGPL-2.1+) - Audio/Video routing

These libraries are dynamically linked and can be replaced by the user.
