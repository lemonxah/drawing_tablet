//! Virtual tablet device implementation using uinput.

use crate::events::*;
use dt_protocol::InputEvent;
use evdev::{
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AbsInfo, AbsoluteAxisType, AttributeSet, BusType, EventType, InputEvent as EvdevEvent, InputId,
    Key, MiscType, PropType, Synchronization, UinputAbsSetup,
};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, info};

/// Configuration for the virtual tablet device identity.
#[derive(Debug, Clone, PartialEq)]
pub struct TabletDescriptor {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
}

impl Default for TabletDescriptor {
    fn default() -> Self {
        Self {
            // Use generic name, no VID/PID (let kernel assign defaults)
            name: "Drawing Tablet Pen".to_string(),
            vid: 0x0000,
            pid: 0x0000,
        }
    }
}

impl TabletDescriptor {
    // Presets removed as we are standardizing on "Drawing Tablet Pen"
}

/// Errors that can occur when creating or using a virtual tablet.
#[derive(Debug, Error)]
pub enum TabletError {
    #[error("failed to create virtual device: {0}")]
    DeviceCreation(#[source] std::io::Error),

    #[error("failed to emit event: {0}")]
    EventEmit(#[source] std::io::Error),

    #[error("invalid touch slot: {slot} (max {max})")]
    InvalidSlot { slot: u8, max: u8 },
}

/// A virtual input system that separates Pen and Touch into two devices.
pub struct VirtualTablet {
    pen_device: VirtualDevice,
    touch_device: VirtualDevice,
    stylus_state: StylusState,
    in_proximity: bool,
    tool_announced: bool, // Track if we've sent the tool serial (only once per session)
    last_pen_activity: Option<Instant>,
    touch_slots: Vec<TouchSlot>,
    current_slot: u8,
    width: u32,
    height: u32,
}

impl VirtualTablet {
    /// Create a new virtual tablet system (Pen + Touchpad).
    ///
    /// The width and height define the coordinate space for input mapping.
    pub fn new(
        width: u32,
        height: u32,
        descriptor: &TabletDescriptor,
    ) -> Result<Self, TabletError> {
        info!(
            "Creating virtual tablet devices ({}x{}) as '{}' (VID: {:04x}, PID: {:04x})",
            width, height, descriptor.name, descriptor.vid, descriptor.pid
        );

        // --- Device 1: Pen (Stylus) ---
        // 65535 / 200mm = ~327 resolution.
        let resolution = 400;

        let abs_x = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_X,
            AbsInfo::new(0, 0, ABS_MAX, 0, 0, resolution),
        );
        let abs_y = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_Y,
            AbsInfo::new(0, 0, ABS_MAX, 0, 0, resolution),
        );
        let abs_pressure = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_PRESSURE,
            AbsInfo::new(0, 0, PRESSURE_MAX, 0, 0, 0),
        );
        let abs_tilt_x = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_TILT_X,
            AbsInfo::new(0, -TILT_MAX, TILT_MAX, 0, 0, 0),
        );
        let abs_tilt_y = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_TILT_Y,
            AbsInfo::new(0, -TILT_MAX, TILT_MAX, 0, 0, 0),
        );
        // Add ABS_DISTANCE (0-255) for hover detection support
        let abs_distance = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_DISTANCE,
            AbsInfo::new(0, 0, 255, 0, 0, 0),
        );

        // Add MSC_SERIAL to identify the tool
        let mut msc_supported = AttributeSet::<MiscType>::new();
        msc_supported.insert(MiscType::MSC_SERIAL);

        let mut pen_keys = AttributeSet::<Key>::new();
        pen_keys.insert(Key::BTN_TOOL_PEN);
        pen_keys.insert(Key::BTN_TOOL_RUBBER); // Eraser support
        pen_keys.insert(Key::BTN_TOUCH); // Often needed for pen contact
        pen_keys.insert(Key::BTN_STYLUS);
        pen_keys.insert(Key::BTN_STYLUS2);

        // Pen Properties: DIRECT + POINTER (OpenTabletDriver style)
        // DIRECT: Indicates direct-input device where coordinates map 1:1 to screen
        // POINTER: Indicates device has pointer capability
        // OTD sets both for their "Artist Tablet" mode
        let mut pen_props = AttributeSet::<PropType>::new();
        pen_props.insert(PropType::DIRECT);
        pen_props.insert(PropType::POINTER);

        // Use BUS_VIRTUAL since this is a virtual device (OTD style)
        // This is more honest than pretending to be a USB device
        let pen_id = InputId::new(BusType::BUS_VIRTUAL, descriptor.vid, descriptor.pid, 0x0100);

        let pen_device = VirtualDeviceBuilder::new()
            .map_err(TabletError::DeviceCreation)?
            .name(&descriptor.name)
            .input_id(pen_id)
            .with_keys(&pen_keys)
            .map_err(TabletError::DeviceCreation)?
            .with_properties(&pen_props)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_x)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_y)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_pressure)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_tilt_x)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_tilt_y)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&abs_distance)
            .map_err(TabletError::DeviceCreation)?
            .with_msc(&msc_supported)
            .map_err(TabletError::DeviceCreation)?
            .build()
            .map_err(TabletError::DeviceCreation)?;

        // --- Device 2: Touchpad (Multitouch) ---
        // Match Pen resolution
        let mt_slot = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_MT_SLOT,
            AbsInfo::new(0, 0, (MT_SLOTS - 1) as i32, 0, 0, 0),
        );
        let mt_tracking_id = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_MT_TRACKING_ID,
            AbsInfo::new(0, 0, 65535, 0, 0, 0),
        );
        let mt_position_x = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_MT_POSITION_X,
            AbsInfo::new(0, 0, ABS_MAX, 0, 0, resolution),
        );
        let mt_position_y = UinputAbsSetup::new(
            AbsoluteAxisType::ABS_MT_POSITION_Y,
            AbsInfo::new(0, 0, ABS_MAX, 0, 0, resolution),
        );

        // Legacy Single-Touch Axes REMOVED
        // We previously added ABS_X/ABS_Y to help GIMP detection, but this causes the OS
        // to treat the device as a "Core Pointer" (Mouse), forcing mouse emulation (selection)
        // even when Touch is disabled. We revert to pure Multitouch to avoid this.

        let mut touch_keys = AttributeSet::<Key>::new();
        // REMOVED ALL BUTTONS TO PREVENT "TOOL" CLASSIFICATION
        // touch_keys.insert(Key::BTN_LEFT);
        // touch_keys.insert(Key::BTN_TOOL_FINGER);
        // touch_keys.insert(Key::BTN_TOOL_DOUBLETAP);
        // touch_keys.insert(Key::BTN_TOOL_TRIPLETAP);
        // touch_keys.insert(Key::BTN_TOOL_QUADTAP);

        // Touch Property: DIRECT (Touchscreen behavior)
        let mut touch_props = AttributeSet::<PropType>::new();
        touch_props.insert(PropType::DIRECT);
        // touch_props.insert(PropType::POINTER);

        // Use the same VID/PID as the Pen device so they are associated as the same physical hardware.
        let touch_id = InputId::new(BusType::BUS_USB, descriptor.vid, descriptor.pid, 0x0101);

        let touch_device = VirtualDeviceBuilder::new()
            .map_err(TabletError::DeviceCreation)?
            .name("Drawing Tablet Touch")
            .input_id(touch_id)
            .with_keys(&touch_keys)
            .map_err(TabletError::DeviceCreation)?
            .with_properties(&touch_props)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&mt_slot)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&mt_tracking_id)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&mt_position_x)
            .map_err(TabletError::DeviceCreation)?
            .with_absolute_axis(&mt_position_y)
            .map_err(TabletError::DeviceCreation)?
            // .with_absolute_axis(&abs_x_legacy) // REMOVED
            // .map_err(TabletError::DeviceCreation)?
            // .with_absolute_axis(&abs_y_legacy) // REMOVED
            // .map_err(TabletError::DeviceCreation)?
            .build()
            .map_err(TabletError::DeviceCreation)?;

        info!("Virtual tablet devices created successfully");

        Ok(Self {
            pen_device,
            touch_device,
            stylus_state: StylusState::default(),
            in_proximity: false,
            tool_announced: false,
            last_pen_activity: None,
            touch_slots: (0..MT_SLOTS).map(|_| TouchSlot::default()).collect(),
            current_slot: 0,
            width,
            height,
        })
    }

    /// Get the coordinate space dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Process an input event from the protocol.
    pub fn process_event(&mut self, event: &InputEvent) -> Result<(), TabletError> {
        // Simple Palm Rejection:
        // If the pen has been active recently (hovering or touching), ignore all touch input.
        // This prevents palm touches while writing.
        const PALM_REJECTION_TIMEOUT: Duration = Duration::from_millis(1000);

        match event {
            InputEvent::StylusMove { .. }
            | InputEvent::StylusDown { .. }
            | InputEvent::StylusUp
            | InputEvent::StylusButton { .. } => {
                // If this is the FIRST pen event after a while, we should forcibly release any touches
                // to prevent "stuck" clicks if the user rests their palm while a finger was still down.
                let was_active = self
                    .last_pen_activity
                    .map_or(false, |t| t.elapsed() < PALM_REJECTION_TIMEOUT);
                self.last_pen_activity = Some(Instant::now());

                if !was_active {
                    // Force release all active touches
                    self.release_all_touches()?;
                }

                match event {
                    InputEvent::StylusMove { .. } => self.stylus_move(event),
                    InputEvent::StylusDown { .. } => self.stylus_down(event),
                    InputEvent::StylusUp => self.stylus_up(),
                    InputEvent::StylusButton { button, pressed } => {
                        self.stylus_button(*button, *pressed)
                    }
                    _ => Ok(()),
                }
            }
            InputEvent::TouchDown { id, x, y } => {
                if let Some(last) = self.last_pen_activity {
                    if last.elapsed() < PALM_REJECTION_TIMEOUT {
                        debug!("Touch suppressed (Palm Rejection)");
                        return Ok(());
                    }
                }
                self.touch_down(*id, *x, *y)
            }
            InputEvent::TouchMove { id, x, y } => {
                if let Some(last) = self.last_pen_activity {
                    if last.elapsed() < PALM_REJECTION_TIMEOUT {
                        // If we are here, it means we might have suppressed the Down event,
                        // so this Move event refers to a non-existent slot.
                        return Ok(());
                    }
                }
                self.touch_move(*id, *x, *y)
            }
            InputEvent::TouchUp { id } => {
                // Always process Up to clean up slots, even if we are in rejection mode.
                // This ensures our internal state tracks reality as much as possible,
                // although the "force release" above handles the OS side.
                self.touch_up(*id)
            }
        }
    }

    /// Force release all active touch slots.
    fn release_all_touches(&mut self) -> Result<(), TabletError> {
        let active_slots: Vec<u8> = self
            .touch_slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active)
            .map(|(i, _)| i as u8)
            .collect();

        if active_slots.is_empty() {
            return Ok(());
        }

        info!(
            "Palm Rejection triggered: Releasing {} active touches",
            active_slots.len()
        );

        for slot in active_slots {
            self.select_slot(slot)?;
            self.touch_slots[slot as usize].active = false;

            self.emit_touch(&[EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
                -1,
            )])?;
        }

        // Reset tools and BTN_TOUCH
        self.emit_touch(&[
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_FINGER.0, 0),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_DOUBLETAP.0, 0),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_TRIPLETAP.0, 0),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_QUADTAP.0, 0),
            // EvdevEvent::new(EventType::KEY, Key::BTN_TOUCH.0, 0), // Removed
        ])?;

        self.sync_touch()
    }

    fn stylus_move(&mut self, event: &InputEvent) -> Result<(), TabletError> {
        self.stylus_state.update_from_event(event);

        // Ensure proximity is established first
        if !self.in_proximity {
            self.enter_proximity()?;
        }

        self.emit_stylus_position()?;
        self.sync_pen()
    }

    fn stylus_down(&mut self, event: &InputEvent) -> Result<(), TabletError> {
        debug!("Stylus down");
        self.stylus_state.update_from_event(event);

        // Ensure proximity is established first (and synced!)
        if !self.in_proximity {
            self.enter_proximity()?;
        }

        // Force minimum pressure to 1 to ensure Krita registers the stroke start.
        // If pressure is 0, apps often treat it as a hover even if BTN_TOUCH is 1.
        let pressure = self.stylus_state.pressure.max(1);

        // Send BTN_TOUCH=1 and Pressure/Position in the same frame.
        self.emit_pen(&[
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_PEN.0, 1), // Re-affirm tool
            EvdevEvent::new(EventType::KEY, Key::BTN_TOUCH.0, 1),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_X.0,
                self.stylus_state.x,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_Y.0,
                self.stylus_state.y,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_PRESSURE.0,
                pressure,
            ),
            // Distance = 0 for touch
            EvdevEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_DISTANCE.0, 0),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_TILT_X.0,
                self.stylus_state.tilt_x,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_TILT_Y.0,
                self.stylus_state.tilt_y,
            ),
        ])?;
        self.sync_pen()
        // We stay in_proximity = true because the pen is likely still hovering
    }

    fn stylus_up(&mut self) -> Result<(), TabletError> {
        debug!("Stylus up");
        self.stylus_state.is_down = false;
        self.stylus_state.pressure = 0;

        self.emit_pen(&[
            EvdevEvent::new(EventType::KEY, Key::BTN_TOUCH.0, 0),
            EvdevEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_PRESSURE.0, 0),
        ])?;
        self.sync_pen()
        // We stay in_proximity = true because the pen is likely still hovering
    }

    fn enter_proximity(&mut self) -> Result<(), TabletError> {
        debug!(
            "Stylus entering proximity (tool_announced={})",
            self.tool_announced
        );

        // Use a realistic serial number for the pen tool.
        // Wacom uses unique serials per physical pen - we use a fixed one.
        // The serial helps apps (like Krita) identify and track individual tools.
        // IMPORTANT: Only send MSC_SERIAL ONCE per session to avoid creating duplicate
        // tablet_tool objects in the Wayland compositor (zwp_tablet_v2 protocol).
        const PEN_SERIAL: i32 = 0x12345678;

        let mut events = vec![EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_PEN.0, 1)];

        // Only send serial on first proximity to register the tool once
        if !self.tool_announced {
            events.push(EvdevEvent::new(
                EventType::MISC,
                MiscType::MSC_SERIAL.0,
                PEN_SERIAL,
            ));
            self.tool_announced = true;
            info!("Announcing tablet tool with serial {:08x}", PEN_SERIAL);
        }

        // Include current coordinates so the tool appears at the right spot immediately
        events.extend([
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_X.0,
                self.stylus_state.x,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_Y.0,
                self.stylus_state.y,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_DISTANCE.0,
                10, // Hovering
            ),
        ]);

        self.emit_pen(&events)?;
        self.sync_pen()?;
        self.in_proximity = true;
        Ok(())
    }

    fn emit_stylus_position(&mut self) -> Result<(), TabletError> {
        // Just emit position/pressure updates.
        // We DO NOT re-emit BTN_TOOL_PEN or MSC_SERIAL here to keep the event stream clean.

        // IMPORTANT: Force pressure to 0 when hovering (!is_down) to prevent false strokes.
        // This is a safety net in case the client sends non-zero pressure during hover.
        // Krita and other apps interpret any pressure > 0 as drawing intent.
        let pressure = if self.stylus_state.is_down {
            self.stylus_state.pressure
        } else {
            0
        };

        self.emit_pen(&[
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_X.0,
                self.stylus_state.x,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_Y.0,
                self.stylus_state.y,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_PRESSURE.0,
                pressure,
            ),
            // If touching, Distance is 0. If hovering, it's > 0 (e.g., 10).
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_DISTANCE.0,
                if self.stylus_state.is_down { 0 } else { 10 },
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_TILT_X.0,
                self.stylus_state.tilt_x,
            ),
            EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_TILT_Y.0,
                self.stylus_state.tilt_y,
            ),
        ])
    }

    fn stylus_button(&mut self, button: u8, pressed: bool) -> Result<(), TabletError> {
        debug!(
            "Stylus button {} {}",
            button,
            if pressed { "pressed" } else { "released" }
        );
        let key = match button {
            0 => Key::BTN_STYLUS,
            1 => Key::BTN_STYLUS2,
            _ => return Ok(()), // Ignore unknown buttons
        };

        // Ensure proximity is established first (this will announce the tool if needed)
        if !self.in_proximity {
            self.enter_proximity()?;
        }

        // Just send the button state - no need to re-send BTN_TOOL_PEN or MSC_SERIAL
        self.emit_pen(&[EvdevEvent::new(
            EventType::KEY,
            key.0,
            if pressed { 1 } else { 0 },
        )])?;
        self.sync_pen()
    }

    fn find_slot_for_id(&mut self, id: u32) -> Option<u8> {
        self.touch_slots
            .iter()
            .position(|s| s.active && s.tracking_id == id as i32)
            .map(|i| i as u8)
    }

    fn find_free_slot(&mut self) -> Option<u8> {
        self.touch_slots
            .iter()
            .position(|s| !s.active)
            .map(|i| i as u8)
    }

    fn touch_down(&mut self, id: u32, x: f32, y: f32) -> Result<(), TabletError> {
        let slot_idx = match self.find_free_slot() {
            Some(s) => s,
            None => return Ok(()), // No free slots, ignore
        };

        debug!(
            "Touch down (Internal): id={} slot={} pos=({}, {})",
            id, slot_idx, x, y
        );

        let abs_x = normalize_to_abs(x, ABS_MAX);
        let abs_y = normalize_to_abs(y, ABS_MAX);

        self.touch_slots[slot_idx as usize] = TouchSlot {
            tracking_id: id as i32,
            x: abs_x,
            y: abs_y,
            active: true,
            reported_to_os: false,
        };

        self.sync_touch_state_to_os()
    }

    fn touch_move(&mut self, id: u32, x: f32, y: f32) -> Result<(), TabletError> {
        let slot_idx = match self.find_slot_for_id(id) {
            Some(s) => s,
            None => return Ok(()), // Unknown touch, ignore
        };

        let abs_x = normalize_to_abs(x, ABS_MAX);
        let abs_y = normalize_to_abs(y, ABS_MAX);

        self.touch_slots[slot_idx as usize].x = abs_x;
        self.touch_slots[slot_idx as usize].y = abs_y;

        self.sync_touch_state_to_os()
    }

    fn touch_up(&mut self, id: u32) -> Result<(), TabletError> {
        let slot_idx = match self.find_slot_for_id(id) {
            Some(s) => s,
            None => return Ok(()), // Unknown touch, ignore
        };

        debug!("Touch up (Internal): id={} slot={}", id, slot_idx);

        self.touch_slots[slot_idx as usize].active = false;

        // We do NOT reset reported_to_os here, because sync_touch_state_to_os
        // needs to see that it WAS reported so it can correctly send the Lift event (Tracking ID -1).

        self.sync_touch_state_to_os()
    }

    /// Sync the internal touch slot state to the OS
    fn sync_touch_state_to_os(&mut self) -> Result<(), TabletError> {
        let mut something_changed = false;

        // Iterate all slots and sync changes
        for i in 0..MT_SLOTS {
            // Extract state needed for decision
            let (is_active_internal, tracking_id, x, y, reported) = {
                let s = &self.touch_slots[i as usize];
                (s.active, s.tracking_id, s.x, s.y, s.reported_to_os)
            };

            // In Direct/Touchscreen mode, we report everything active
            let want_active_os = is_active_internal;

            // --- GESTURE-ONLY FILTER ---
            // Only report to OS if we have at least 2 active touches internally
            // This prevents single-finger interactions (like drawing/selecting) from leaking through.
            let active_internal_count = self.touch_slots.iter().filter(|s| s.active).count();
            if active_internal_count < 2 {
                // Force everything to be HIDDEN if less than 2 fingers
                if reported {
                    // SHOWING -> HIDDEN
                    self.select_slot(i)?;
                    self.emit_touch(&[EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
                        -1,
                    )])?;
                    self.touch_slots[i as usize].reported_to_os = false;
                    something_changed = true;
                }
                continue; // Skip the rest of the loop for this slot
            }

            // Legacy ABS_X/ABS_Y removed to prevent mouse emulation

            if !reported && want_active_os {
                // HIDDEN -> SHOWING
                self.select_slot(i)?;
                self.emit_touch(&[
                    EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
                        tracking_id,
                    ),
                    EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_POSITION_X.0,
                        x,
                    ),
                    EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_POSITION_Y.0,
                        y,
                    ),
                ])?;
                self.touch_slots[i as usize].reported_to_os = true;
                something_changed = true;
            } else if reported && !want_active_os {
                // SHOWING -> HIDDEN
                self.select_slot(i)?;
                self.emit_touch(&[EvdevEvent::new(
                    EventType::ABSOLUTE,
                    AbsoluteAxisType::ABS_MT_TRACKING_ID.0,
                    -1,
                )])?;
                self.touch_slots[i as usize].reported_to_os = false;
                something_changed = true;
            } else if reported && want_active_os {
                // SHOWING -> SHOWING (Update Position)
                self.select_slot(i)?;
                self.emit_touch(&[
                    EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_POSITION_X.0,
                        x,
                    ),
                    EvdevEvent::new(
                        EventType::ABSOLUTE,
                        AbsoluteAxisType::ABS_MT_POSITION_Y.0,
                        y,
                    ),
                ])?;
                something_changed = true;
            }
        }

        if something_changed {
            // Calculate how many slots are NOW reported to OS
            let reported_count = self.touch_slots.iter().filter(|s| s.reported_to_os).count();

            self.update_touch_tools(reported_count)?;

            // BTN_TOUCH logic:
            // For a "Touchscreen", BTN_TOUCH should be 1 if ANY finger is down.
            // REMOVED: To prevent mouse emulation. We rely on ABS_MT_TRACKING_ID and BTN_TOOL_*
            /*
            let btn_touch_value = if reported_count > 0 { 1 } else { 0 };
            self.emit_touch(&[EvdevEvent::new(
                EventType::KEY,
                Key::BTN_TOUCH.0,
                btn_touch_value,
            )])?;
            */

            self.sync_touch()?;
        }

        Ok(())
    }

    fn update_touch_tools(&mut self, count: usize) -> Result<(), TabletError> {
        let _finger = if count == 1 { 1 } else { 0 };
        let _double = if count == 2 { 1 } else { 0 };
        let _triple = if count == 3 { 1 } else { 0 };
        let _quad = if count >= 4 { 1 } else { 0 };

        /*
        self.emit_touch(&[
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_FINGER.0, finger),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_DOUBLETAP.0, double),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_TRIPLETAP.0, triple),
            EvdevEvent::new(EventType::KEY, Key::BTN_TOOL_QUADTAP.0, quad),
        ])
        */
        Ok(())
    }

    fn select_slot(&mut self, slot: u8) -> Result<(), TabletError> {
        if slot >= MT_SLOTS {
            return Err(TabletError::InvalidSlot {
                slot,
                max: MT_SLOTS - 1,
            });
        }

        if self.current_slot != slot {
            self.current_slot = slot;
            self.emit_touch(&[EvdevEvent::new(
                EventType::ABSOLUTE,
                AbsoluteAxisType::ABS_MT_SLOT.0,
                slot as i32,
            )])?;
        }
        Ok(())
    }

    fn emit_pen(&mut self, events: &[EvdevEvent]) -> Result<(), TabletError> {
        tracing::debug!("Emitting pen events: {:?}", events);
        self.pen_device.emit(events).map_err(TabletError::EventEmit)
    }

    fn sync_pen(&mut self) -> Result<(), TabletError> {
        self.emit_pen(&[EvdevEvent::new(
            EventType::SYNCHRONIZATION,
            Synchronization::SYN_REPORT.0,
            0,
        )])
    }

    fn emit_touch(&mut self, events: &[EvdevEvent]) -> Result<(), TabletError> {
        // tracing::debug!("Emitting touch events: {:?}", events);
        for event in events {
            tracing::debug!(
                "TOUCH EMIT: Type={:?} Code={} Value={}",
                event.kind(),
                event.code(),
                event.value()
            );
        }
        self.touch_device
            .emit(events)
            .map_err(TabletError::EventEmit)
    }

    fn sync_touch(&mut self) -> Result<(), TabletError> {
        self.emit_touch(&[EvdevEvent::new(
            EventType::SYNCHRONIZATION,
            Synchronization::SYN_REPORT.0,
            0,
        )])
    }
}
