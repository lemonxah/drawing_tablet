//! VA-API hardware H.264 encoding using GStreamer.

use dt_capture::{Frame, FrameData, PixelFormat};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_allocators as gst_allocators;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors that can occur during encoding.
#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("GStreamer initialization failed: {0}")]
    GstInit(String),

    #[error("pipeline creation failed: {0}")]
    PipelineCreation(String),

    #[error("element not found: {0}")]
    ElementNotFound(String),

    #[error("pipeline state change failed")]
    StateChange,

    #[error("encoding failed: {0}")]
    EncodingFailed(String),

    #[error("unsupported pixel format: {0:?}")]
    UnsupportedFormat(PixelFormat),

    #[error("encoder stopped")]
    Stopped,
}

/// Configuration for the video encoder.
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video width.
    pub width: u32,
    /// Video height.
    pub height: u32,
    /// Target frame rate.
    pub fps: u32,
    /// Target bitrate in kbps.
    pub bitrate: u32,
    /// Keyframe interval (in frames).
    pub keyframe_interval: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 100,
            bitrate: 25000,
            keyframe_interval: 2,
        }
    }
}

/// An encoded video frame.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Whether this is a keyframe (I-frame).
    pub is_keyframe: bool,
    /// Presentation timestamp in microseconds.
    pub pts: u64,
    /// H.264 NAL unit data.
    pub data: Vec<u8>,
}

/// Video encoder using GStreamer with VA-API.
pub struct VideoEncoder {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    config: EncoderConfig,
    frame_count: u64,
    force_keyframe: bool,
}

impl VideoEncoder {
    /// Create a new video encoder with the given configuration.
    pub fn new(config: EncoderConfig) -> Result<Self, EncoderError> {
        info!(
            "Creating video encoder: {}x{} @ {} fps, {} kbps",
            config.width, config.height, config.fps, config.bitrate
        );

        gst::init().map_err(|e| EncoderError::GstInit(e.to_string()))?;

        // Build the pipeline
        // Try NVENC first, then VA-API, then fall back to x264
        // Note: We skip CPU-based videoconvert for NVENC and upload BGRA directly to GPU
        let nvenc_str = format!(
            "appsrc name=src format=time is-live=true do-timestamp=true \
             caps=video/x-raw,format=BGRA,width={width},height={height},framerate={fps}/1 ! \
             cudaupload ! \
             nvh264enc bitrate={bitrate} gop-size={keyframe} rc-mode=cbr preset=p4 \
             zerolatency=true tune=low-latency \
             aud=true repeat-sequence-header=true ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink emit-signals=true sync=false",
            width = config.width,
            height = config.height,
            fps = config.fps,
            bitrate = config.bitrate,
            keyframe = config.keyframe_interval,
        );

        let pipeline = match gst::parse::launch(&nvenc_str) {
            Ok(p) => {
                info!("Using NVENC encoder (Direct BGRA upload)");
                p
            }
            Err(_) => {
                warn!("NVENC encoder not available, trying VA-API");
                // Use vaapipostproc to handle both DMABUF and System memory (upload)
                let vaapi_str = format!(
                    "appsrc name=src format=time is-live=true do-timestamp=true \
                     caps=video/x-raw,format=BGRA,width={width},height={height},framerate={fps}/1 ! \
                     vaapipostproc ! \
                     video/x-raw,format=NV12 ! \
                     vaapih264enc rate-control=cbr bitrate={bitrate} keyframe-period={keyframe} \
                     tune=low-latency refs=1 num-slices=1 ! \
                     video/x-h264,stream-format=byte-stream,alignment=au ! \
                     appsink name=sink emit-signals=true sync=false",
                    width = config.width,
                    height = config.height,
                    fps = config.fps,
                    bitrate = config.bitrate,
                    keyframe = config.keyframe_interval,
                );

                match gst::parse::launch(&vaapi_str) {
                    Ok(p) => {
                        info!("Using VA-API encoder");
                        p
                    }
                    Err(_) => {
                        warn!("VA-API not available, falling back to x264");
                        let x264_str = format!(
                            "appsrc name=src format=time is-live=true do-timestamp=true \
                             caps=video/x-raw,format=BGRA,width={width},height={height},framerate={fps}/1 ! \
                             videoconvert ! \
                             x264enc tune=zerolatency speed-preset=superfast bitrate={bitrate} \
                             key-int-max={keyframe} b-adapt=0 cabac=true aud=true ! \
                             video/x-h264,stream-format=byte-stream,alignment=au ! \
                             appsink name=sink emit-signals=true sync=false",
                            width = config.width,
                            height = config.height,
                            fps = config.fps,
                            bitrate = config.bitrate,
                            keyframe = config.keyframe_interval,
                        );
                        gst::parse::launch(&x264_str)
                            .map_err(|e| EncoderError::PipelineCreation(e.to_string()))?
                    }
                }
            }
        };

        let pipeline = pipeline
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| EncoderError::PipelineCreation("not a pipeline".to_string()))?;

        let appsrc = pipeline
            .by_name("src")
            .ok_or_else(|| EncoderError::ElementNotFound("appsrc".to_string()))?
            .dynamic_cast::<gst_app::AppSrc>()
            .map_err(|_| EncoderError::ElementNotFound("appsrc cast failed".to_string()))?;

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| EncoderError::ElementNotFound("appsink".to_string()))?
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| EncoderError::ElementNotFound("appsink cast failed".to_string()))?;

        // Start the pipeline
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| EncoderError::StateChange)?;

        info!("Video encoder pipeline started");

        Ok(Self {
            pipeline,
            appsrc,
            appsink,
            config,
            frame_count: 0,
            force_keyframe: false,
        })
    }

    /// Get the encoder configuration.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Request that the next frame be encoded as a keyframe.
    pub fn request_keyframe(&mut self) {
        debug!("Keyframe requested");
        self.force_keyframe = true;
    }

    /// Encode a captured frame.
    pub fn encode(&mut self, frame: &Frame) -> Result<Option<EncodedFrame>, EncoderError> {
        let mut buffer = match &frame.data {
            FrameData::MemPtr(data) => {
                let normalized = self.normalize_frame_data(frame, data)?;
                let mut buf = gst::Buffer::with_size(normalized.len())
                    .map_err(|e| EncoderError::EncodingFailed(e.to_string()))?;

                {
                    let buffer_mut = buf.get_mut().unwrap();
                    let mut map = buffer_mut
                        .map_writable()
                        .map_err(|e| EncoderError::EncodingFailed(e.to_string()))?;
                    map.copy_from_slice(&normalized);
                }
                buf
            }
            FrameData::DmaBuf {
                fd,
                offset,
                stride,
                modifier: _,
            } => {
                // Dup the FD because GStreamer memory will own it
                let fd = unsafe { libc::dup(*fd) };
                if fd < 0 {
                    return Err(EncoderError::EncodingFailed("dup failed".to_string()));
                }

                let size = (*stride * frame.height) as usize;
                let allocator = gst_allocators::DmaBufAllocator::new();
                let memory =
                    unsafe { gst_allocators::DmaBufAllocator::alloc(&allocator, fd, size) }
                        .map_err(|e| {
                            EncoderError::EncodingFailed(format!("alloc failed: {:?}", e))
                        })?;

                let mut buf = gst::Buffer::new();
                buf.get_mut().unwrap().append_memory(memory);

                // Add VideoMeta to describe stride, offset
                let format = match frame.format {
                    PixelFormat::Bgra => gst_video::VideoFormat::Bgra,
                    PixelFormat::Rgba => gst_video::VideoFormat::Rgba,
                    PixelFormat::Bgrx => gst_video::VideoFormat::Bgrx,
                    PixelFormat::Rgbx => gst_video::VideoFormat::Rgbx,
                    _ => gst_video::VideoFormat::Unknown,
                };

                if format != gst_video::VideoFormat::Unknown {
                    if let Ok(meta) = gst_video::VideoMeta::add(
                        buf.get_mut().unwrap(),
                        gst_video::VideoFrameFlags::empty(),
                        format,
                        frame.width,
                        frame.height,
                    ) {
                        if !meta.offset().is_empty() {
                            let ptr = meta.offset().as_ptr() as *mut usize;
                            unsafe { *ptr = *offset as usize };
                        }
                        if !meta.stride().is_empty() {
                            let ptr = meta.stride().as_ptr() as *mut i32;
                            unsafe { *ptr = *stride as i32 };
                        }
                    }
                }

                buf
            }
        };

        {
            let buffer_mut = buffer.get_mut().unwrap();

            // Set timestamp
            let pts = gst::ClockTime::from_useconds(
                self.frame_count * 1_000_000 / self.config.fps as u64,
            );
            buffer_mut.set_pts(pts);
            buffer_mut.set_dts(pts);
            buffer_mut.set_duration(gst::ClockTime::from_useconds(
                1_000_000 / self.config.fps as u64,
            ));
        }

        // Handle keyframe request
        if self.force_keyframe {
            self.force_keyframe = false;
            debug!("Force keyframe event being sent");
            // Send force-keyframe event - using DownstreamForceKeyUnitEvent to reach the encoder
            let event = gst_video::DownstreamForceKeyUnitEvent::builder()
                .all_headers(true)
                .build();

            if !self.appsrc.send_event(event) {
                warn!("Failed to send force-keyframe event");
            }
        }

        // Push buffer to pipeline
        self.appsrc
            .push_buffer(buffer)
            .map_err(|e| EncoderError::EncodingFailed(format!("push failed: {:?}", e)))?;

        self.frame_count += 1;

        // Try to pull encoded frame
        self.pull_encoded_frame()
    }

    fn normalize_frame_data(&self, frame: &Frame, data: &[u8]) -> Result<Vec<u8>, EncoderError> {
        if frame.format == PixelFormat::Unknown {
            return Err(EncoderError::UnsupportedFormat(frame.format));
        }

        let bytes_per_pixel = 4usize;
        let row_len = frame.width as usize * bytes_per_pixel;
        let mut stride = frame.stride as usize;
        let expected_row_data = row_len * frame.height as usize;

        if stride == row_len
            && data.len() > expected_row_data
            && data.len().is_multiple_of(frame.height as usize)
        {
            let inferred = data.len() / frame.height as usize;
            if inferred >= row_len {
                stride = inferred;
            }
        }

        let expected_len = stride * frame.height as usize;

        if data.len() < expected_len {
            return Err(EncoderError::EncodingFailed(format!(
                "frame data too small: got {} bytes, expected at least {}",
                data.len(),
                expected_len
            )));
        }

        if stride == row_len {
            return Ok(self.convert_pixel_format(frame, &data[..expected_row_data]));
        }

        let mut packed = vec![0u8; row_len * frame.height as usize];
        for row in 0..frame.height as usize {
            let src_start = row * stride;
            let src_end = src_start + row_len;
            let dst_start = row * row_len;
            let dst_end = dst_start + row_len;
            packed[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
        }

        Ok(self.convert_pixel_format(frame, &packed))
    }

    fn convert_pixel_format(&self, frame: &Frame, data: &[u8]) -> Vec<u8> {
        match frame.format {
            PixelFormat::Bgra | PixelFormat::Bgrx => data.to_vec(),
            PixelFormat::Rgba | PixelFormat::Rgbx => {
                let mut out = data.to_vec();
                for pixel in out.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                out
            }
            PixelFormat::Unknown => data.to_vec(),
        }
    }

    /// Pull an encoded frame from the pipeline (non-blocking).
    fn pull_encoded_frame(&mut self) -> Result<Option<EncodedFrame>, EncoderError> {
        match self
            .appsink
            .try_pull_sample(gst::ClockTime::from_mseconds(0))
        {
            Some(sample) => {
                let buffer = sample
                    .buffer()
                    .ok_or_else(|| EncoderError::EncodingFailed("no buffer".to_string()))?;

                let map = buffer
                    .map_readable()
                    .map_err(|e| EncoderError::EncodingFailed(e.to_string()))?;

                let pts = buffer.pts().map(|t| t.useconds()).unwrap_or(0);

                // Check if it's a keyframe by looking for NAL unit type
                let is_keyframe = is_h264_keyframe(map.as_slice());

                if is_keyframe {
                    debug!(
                        "Keyframe detected, pts: {}, size: {} bytes",
                        pts,
                        map.as_slice().len()
                    );
                }

                Ok(Some(EncodedFrame {
                    is_keyframe,
                    pts,
                    data: map.as_slice().to_vec(),
                }))
            }
            None => Ok(None),
        }
    }

    /// Flush the encoder and get any remaining frames.
    pub fn flush(&mut self) -> Vec<EncodedFrame> {
        let mut frames = Vec::new();

        // Send EOS
        let _ = self.appsrc.end_of_stream();

        // Pull remaining frames
        while let Some(sample) = self
            .appsink
            .try_pull_sample(gst::ClockTime::from_mseconds(100))
        {
            if let Some(buffer) = sample.buffer() {
                if let Ok(map) = buffer.map_readable() {
                    let pts = buffer.pts().map(|t| t.useconds()).unwrap_or(0);
                    let is_keyframe = is_h264_keyframe(map.as_slice());
                    if is_keyframe {
                        debug!(
                            "Flush keyframe detected, pts: {}, size: {} bytes",
                            pts,
                            map.as_slice().len()
                        );
                    }
                    frames.push(EncodedFrame {
                        is_keyframe,
                        pts,
                        data: map.as_slice().to_vec(),
                    });
                }
            }
        }

        frames
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Check if H.264 data contains keyframe NAL units (SPS, PPS, or IDR).
fn is_h264_keyframe(data: &[u8]) -> bool {
    // Look for NAL unit start codes and check NAL type
    let mut i = 0;
    let mut has_sps = false;
    let mut has_pps = false;
    let mut has_idr = false;

    while i + 4 < data.len() {
        // Look for start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01)
        if data[i] == 0 && data[i + 1] == 0 {
            let nal_start = if data[i + 2] == 1 {
                i + 3
            } else if data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1 {
                i + 4
            } else {
                i += 1;
                continue;
            };

            if nal_start < data.len() {
                let nal_type = data[nal_start] & 0x1F;
                // NAL type 7 = SPS (Sequence Parameter Set) - part of keyframe
                if nal_type == 7 {
                    has_sps = true;
                }
                // NAL type 8 = PPS (Picture Parameter Set) - part of keyframe
                else if nal_type == 8 {
                    has_pps = true;
                }
                // NAL type 5 = IDR (Instantaneous Decoder Refresh) - keyframe
                else if nal_type == 5 {
                    has_idr = true;
                }

                // If we have any keyframe markers, consider it a keyframe
                if has_idr || (has_sps && has_pps) {
                    return true;
                }
            }
        }
        i += 1;
    }

    // Return true if we found any keyframe markers
    has_idr || has_sps || has_pps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyframe_detection() {
        // IDR NAL unit
        let idr = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88];
        assert!(is_h264_keyframe(idr));

        // SPS NAL unit
        let sps = &[0x00, 0x00, 0x01, 0x67, 0x42];
        assert!(is_h264_keyframe(sps));

        // Non-IDR slice
        let non_idr = &[0x00, 0x00, 0x00, 0x01, 0x41, 0x9A];
        assert!(!is_h264_keyframe(non_idr));
    }
}
