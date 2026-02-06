//! Event type conversions and constants.

use dt_protocol::InputEvent;

/// Maximum absolute coordinate value for tablet axes.
pub const ABS_MAX: i32 = 65535;

/// Maximum pressure value.
pub const PRESSURE_MAX: i32 = 2047;

/// Maximum tilt value in degrees.
pub const TILT_MAX: i32 = 90;

/// Maximum number of multitouch slots.
pub const MT_SLOTS: u8 = 10;

/// Convert normalized coordinates (0.0-1.0) to absolute coordinates.
#[inline]
pub fn normalize_to_abs(value: f32, max: i32) -> i32 {
    (value.clamp(0.0, 1.0) * max as f32) as i32
}

/// Convert tilt degrees (-90 to 90) to absolute values.
#[inline]
pub fn tilt_to_abs(degrees: f32) -> i32 {
    degrees.clamp(-TILT_MAX as f32, TILT_MAX as f32) as i32
}

/// Internal representation of stylus state.
#[derive(Debug, Clone, Default)]
pub struct StylusState {
    pub x: i32,
    pub y: i32,
    pub pressure: i32,
    pub tilt_x: i32,
    pub tilt_y: i32,
    pub is_down: bool,
}

impl StylusState {
    /// Update from a protocol input event, returning whether the stylus is touching.
    pub fn update_from_event(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::StylusMove { x, y, pressure, tilt_x, tilt_y }
            | InputEvent::StylusDown { x, y, pressure, tilt_x, tilt_y } => {
                self.x = normalize_to_abs(*x, ABS_MAX);
                self.y = normalize_to_abs(*y, ABS_MAX);
                self.pressure = normalize_to_abs(*pressure, PRESSURE_MAX);
                self.tilt_x = tilt_to_abs(*tilt_x);
                self.tilt_y = tilt_to_abs(*tilt_y);

                if matches!(event, InputEvent::StylusDown { .. }) {
                    self.is_down = true;
                }
                self.is_down
            }
            InputEvent::StylusUp => {
                self.is_down = false;
                self.pressure = 0;
                false
            }
            _ => self.is_down,
        }
    }
}

/// Internal representation of a touch slot.
#[derive(Debug, Clone, Default)]
pub struct TouchSlot {
    pub tracking_id: i32,
    pub x: i32,
    pub y: i32,
    pub active: bool,
}
