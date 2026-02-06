//! PipeWire screen capture implementation.

use crate::portal::MonitorInfo;
use drm_fourcc::{DrmFourcc, DrmModifier};
use pipewire::context::Context;
use pipewire::main_loop::MainLoop;
use pipewire::properties::properties;
use pipewire::spa::param::video::VideoFormat;
use pipewire::spa::pod::serialize::GenError;
use pipewire::spa::pod::{ChoiceValue, Object, Property, PropertyFlags, Value};
use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Direction};
use pipewire::spa::{self};
use pipewire::stream::{Stream, StreamFlags};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{error, info};

/// Errors that can occur during screen capture.
#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("PipeWire initialization failed: {0}")]
    InitFailed(String),

    #[error("stream connection failed: {0}")]
    StreamFailed(String),

    #[error("capture stopped")]
    Stopped,

    #[error("no frame data available")]
    NoFrameData,
}

/// Pixel format of captured frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Rgba,
    Bgrx,
    Rgbx,
    Unknown,
}

impl From<VideoFormat> for PixelFormat {
    fn from(format: VideoFormat) -> Self {
        match format {
            VideoFormat::BGRA => Self::Bgra,
            VideoFormat::RGBA => Self::Rgba,
            VideoFormat::BGRx => Self::Bgrx,
            VideoFormat::RGBx => Self::Rgbx,
            _ => Self::Unknown,
        }
    }
}

/// Frame data types.
#[derive(Debug, Clone)]
pub enum FrameData {
    /// DMA-BUF file descriptor with modifier.
    DmaBuf {
        fd: i32,
        offset: u32,
        stride: u32,
        modifier: u64,
    },
    /// Memory-mapped data (copy).
    MemPtr(Vec<u8>),
}

impl Drop for FrameData {
    fn drop(&mut self) {
        if let FrameData::DmaBuf { fd, .. } = self {
            if *fd >= 0 {
                unsafe { libc::close(*fd) };
            }
        }
    }
}

/// A captured frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// Stride (bytes per row).
    pub stride: u32,
    /// Frame data.
    pub data: FrameData,
}

/// Screen capture handle.
pub struct ScreenCapture {
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    frame_rx: Option<mpsc::Receiver<Frame>>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl ScreenCapture {
    /// Start capturing from the specified monitor.
    ///
    /// Frames will be delivered via the returned receiver.
    pub fn start(monitor: &MonitorInfo) -> Result<Self, CaptureError> {
        let node_id = monitor.node_id;
        let width = monitor.width;
        let height = monitor.height;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let (frame_tx, frame_rx) = mpsc::sync_channel(10); // Larger buffer to handle burst processing

        let stop_flag_clone = stop_flag.clone();
        let pause_flag_clone = pause_flag.clone();

        let thread_handle = thread::spawn(move || {
            if let Err(e) = run_capture_loop(
                node_id,
                width,
                height,
                stop_flag_clone,
                pause_flag_clone,
                frame_tx,
            ) {
                error!("Capture loop error: {}", e);
            }
        });

        Ok(Self {
            stop_flag,
            pause_flag,
            frame_rx: Some(frame_rx),
            thread_handle: Some(thread_handle),
        })
    }

    /// Take the frame receiver channel.
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<Frame>> {
        self.frame_rx.take()
    }

    /// Get the next captured frame.
    ///
    /// Returns `None` if no frame is available yet or receiver was taken.
    pub fn try_recv_frame(&self) -> Option<Frame> {
        self.frame_rx.as_ref()?.try_recv().ok()
    }

    /// Wait for the next captured frame.
    pub fn recv_frame(&self) -> Result<Frame, CaptureError> {
        self.frame_rx
            .as_ref()
            .ok_or(CaptureError::Stopped)?
            .recv()
            .map_err(|_| CaptureError::Stopped)
    }

    /// Pause capturing.
    pub fn pause(&self) {
        self.pause_flag.store(true, Ordering::SeqCst);
    }

    /// Resume capturing.
    pub fn resume(&self) {
        self.pause_flag.store(false, Ordering::SeqCst);
    }

    /// Stop capturing and release resources.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        info!("ScreenCapture::drop called");
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            info!("Waiting for capture thread to join...");
            let _ = handle.join();
            info!("Capture thread joined");
        }
    }
}

fn run_capture_loop(
    node_id: u32,
    _width: u32,
    _height: u32,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    frame_tx: mpsc::SyncSender<Frame>,
) -> Result<(), CaptureError> {
    info!("Starting PipeWire capture loop for node {}", node_id);

    pipewire::init();

    let mainloop = MainLoop::new(None).map_err(|e| CaptureError::InitFailed(e.to_string()))?;
    let context = Context::new(&mainloop).map_err(|e| CaptureError::InitFailed(e.to_string()))?;
    let core = context
        .connect(None)
        .map_err(|e| CaptureError::InitFailed(e.to_string()))?;

    // ... (rest of setup)

    // ... (stream creation) ...

    let stream = Stream::new(
        &core,
        "drawing-tablet-capture",
        properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| CaptureError::StreamFailed(e.to_string()))?;

    let frame_tx_clone = frame_tx.clone();
    let pause_flag_clone = pause_flag.clone();
    let width_clone = _width;
    let height_clone = _height;

    let _listener = stream
        .add_local_listener()
        .process(move |stream, _input: &mut ()| {
            if pause_flag_clone.load(Ordering::SeqCst) {
                return;
            }

            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                let data = &mut datas[0];
                let (offset, size, stride) = {
                    let chunk = data.chunk();
                    (
                        chunk.offset() as usize,
                        chunk.size() as usize,
                        chunk.stride() as u32,
                    )
                };

                // We rely on MAP_BUFFERS to give us a mapped slice
                if let Some(slice) = data.data() {
                    let slice: &[u8] = slice;

                    if offset + size <= slice.len() {
                        let frame_data = slice[offset..offset + size].to_vec();

                        let frame = Frame {
                            width: width_clone,
                            height: height_clone,
                            format: PixelFormat::Bgra, // Assuming BGRA for now
                            stride,
                            data: FrameData::MemPtr(frame_data),
                        };

                        let _ = frame_tx_clone.try_send(frame);
                    }
                }
            }
        })
        .register();

    let params_obj = get_format_params(None);
    let params_bytes =
        obj_to_bytes(params_obj).map_err(|e| CaptureError::InitFailed(e.to_string()))?;
    let pod = pipewire::spa::pod::Pod::from_bytes(&params_bytes)
        .ok_or_else(|| CaptureError::InitFailed("Failed to parse format params".to_string()))?;

    stream
        .connect(
            Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut [pod],
        )
        .map_err(|e| CaptureError::StreamFailed(e.to_string()))?;

    info!("Entering capture loop");
    while !stop_flag.load(Ordering::SeqCst) {
        // trace!("Iterating main loop...");
        mainloop.loop_().iterate(Duration::from_millis(100));
    }

    info!("Capture loop stopped, drop flag set");
    // Explicitly destroy stream/core/context/mainloop?
    // They are dropped when function returns.

    Ok(())
}

// Helpers adapted from wlx-capture

fn obj_to_bytes(obj: Object) -> Result<Vec<u8>, GenError> {
    Ok(pipewire::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pipewire::spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner())
}

fn get_format_params(fmt: Option<(&DrmFourcc, &Vec<DrmModifier>)>) -> Object {
    let mut obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
    );

    if let Some(fmt) = fmt {
        let spa_fmt = fourcc_to_spa(*fmt.0);
        let prop = spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa_fmt,
            spa_fmt,
        );
        obj.properties.push(prop);

        let prop = Property {
            key: spa::param::format::FormatProperties::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY | PropertyFlags::from_bits_truncate(16), // DONT_FIXATE
            value: Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: u64::from(fmt.1[0]) as _,
                    alternatives: fmt.1.iter().map(|m| u64::from(*m) as _).collect(),
                },
            ))),
        };
        obj.properties.push(prop);
    } else {
        // Generic fallback
        let prop = spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRA,
            VideoFormat::RGBA,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
        );
        obj.properties.push(prop);
    }

    obj
}

fn fourcc_to_spa(fourcc: DrmFourcc) -> VideoFormat {
    match fourcc {
        DrmFourcc::Argb8888 => VideoFormat::BGRA,
        DrmFourcc::Abgr8888 => VideoFormat::RGBA,
        DrmFourcc::Xrgb8888 => VideoFormat::BGRx,
        DrmFourcc::Xbgr8888 => VideoFormat::RGBx,
        DrmFourcc::Abgr2101010 => VideoFormat::ABGR_210LE,
        DrmFourcc::Xbgr2101010 => VideoFormat::xBGR_210LE,
        _ => panic!("Unsupported format"),
    }
}
