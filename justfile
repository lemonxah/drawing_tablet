# Drawing Tablet Project Commands

# Default recipe - show available commands
default:
    @just --list

# === Rust Server ===

# Build the server (debug)
build:
    cargo build

# Build the server (release)
build-release:
    cargo build --release

# Run all tests
test:
    cargo test

# Run the server (debug)
run:
    cargo run --bin dt-server

# Run the server (release)
run-release:
    cargo run --release --bin dt-server

# Run the server with custom options
run-with port="9999" fps="60" bitrate="8000":
    cargo run --bin dt-server -- --port {{port}} --fps {{fps}} --bitrate {{bitrate}}

# Check code without building
check:
    cargo check

# Format code
fmt:
    cargo fmt

# Run clippy lints
clippy:
    cargo clippy -- -W clippy::all

# Clean build artifacts
clean:
    cargo clean

# === Android App ===

# Build Android app (debug)
android-build:
    cd android && ./gradlew assembleDebug

# Build Android app (release)
android-build-release:
    cd android && ./gradlew assembleRelease

# Install Android app via ADB (debug)
android-install:
    cd android && ./gradlew installDebug

# Install APK directly via ADB
android-adb-install:
    adb install -r android/app/build/outputs/apk/debug/app-debug.apk

# Uninstall Android app
android-uninstall:
    adb uninstall com.drawingtablet

# Build and install Android app
android-deploy: android-build android-adb-install

# Clean Android build
android-clean:
    cd android && ./gradlew clean

# View Android logs
android-logs:
    adb logcat -s "DrawingTablet:*" "UdpClient:*" "VideoDecoder:*" "InputCapture:*"

# === Development ===

# Watch for changes and rebuild
watch:
    cargo watch -x check

# Run server with debug logging
run-debug:
    RUST_LOG=debug cargo run --bin dt-server

# Run server with trace logging
run-trace:
    RUST_LOG=trace cargo run --bin dt-server

# === Testing ===

# Test virtual tablet with evtest (requires sudo)
test-tablet:
    @echo "Creating virtual tablet - check with: sudo evtest"
    @echo "Press Ctrl+C to stop"
    cargo run --example test_tablet 2>/dev/null || echo "Note: Create examples/test_tablet.rs to test"

# Verify GStreamer VA-API is available
check-vaapi:
    gst-inspect-1.0 vaapih264enc || echo "VA-API encoder not found, will use x264 fallback"

# Check PipeWire is running
check-pipewire:
    pw-cli info 0 || echo "PipeWire not running"

# === Full Workflow ===

# Build everything
all: build android-build

# Clean everything
clean-all: clean android-clean

# Full release build
release: build-release android-build-release
