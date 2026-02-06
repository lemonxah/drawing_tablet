use crate::events::{TouchSlot, ABS_MAX, MT_SLOTS};
use std::f32::consts::PI;

/// A simple gesture detector that calculates Pan, Zoom, and Rotate deltas
/// from multitouch data.
pub struct GestureDetector {
    /// Previous positions of active slots.
    /// We use tracking_id to ensure we match the correct fingers.
    previous_points: Vec<Option<(i32, f32, f32)>>, // (tracking_id, x, y)
}

#[derive(Debug, Default)]
pub struct GestureDelta {
    pub pan_x: f32,  // -1.0 to 1.0 (relative to screen size)
    pub pan_y: f32,  // -1.0 to 1.0
    pub zoom: f32,   // Scale factor delta (e.g. 0.01 = 1% zoom in)
    pub rotate: f32, // Radians
}

impl Default for GestureDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureDetector {
    pub fn new() -> Self {
        Self {
            previous_points: vec![None; MT_SLOTS as usize],
        }
    }

    /// Process the current state of touch slots and calculate movement deltas.
    /// Returns None if < 2 fingers are active.
    pub fn process(&mut self, slots: &[TouchSlot]) -> Option<GestureDelta> {
        // 1. Collect current active points
        let current_points: Vec<(usize, f32, f32)> = slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active)
            .map(|(i, s)| (i, s.x as f32, s.y as f32))
            .collect();

        if current_points.len() < 2 {
            // Not enough fingers for gestures. Reset state.
            self.update_previous(slots);
            return None;
        }

        // 2. Match with previous points to calculate deltas
        // We calculate the centroid of the current points and previous points
        // Only use points that were active in BOTH frames
        let mut matched_count = 0;
        let mut prev_center_x = 0.0;
        let mut prev_center_y = 0.0;
        let mut curr_center_x = 0.0;
        let mut curr_center_y = 0.0;

        let mut matched_pairs = Vec::new();

        for (idx, curr_x, curr_y) in &current_points {
            if let Some(Some((prev_id, prev_x, prev_y))) = self.previous_points.get(*idx) {
                if *prev_id == slots[*idx].tracking_id {
                    // Found a match
                    prev_center_x += prev_x;
                    prev_center_y += prev_y;
                    curr_center_x += curr_x;
                    curr_center_y += curr_y;
                    matched_pairs.push((*prev_x, *prev_y, *curr_x, *curr_y));
                    matched_count += 1;
                }
            }
        }

        if matched_count < 2 {
            // Need at least 2 matched fingers to calculate Zoom/Rotate
            self.update_previous(slots);
            return None;
        }

        // Calculate Centroids
        prev_center_x /= matched_count as f32;
        prev_center_y /= matched_count as f32;
        curr_center_x /= matched_count as f32;
        curr_center_y /= matched_count as f32;

        // --- PAN ---
        // Normalize pan to 0.0-1.0 range
        let screen_size = ABS_MAX as f32;
        let delta_x = (curr_center_x - prev_center_x) / screen_size;
        let delta_y = (curr_center_y - prev_center_y) / screen_size;

        // --- ZOOM & ROTATE ---
        // We compare the angle and distance of each point relative to the centroid
        let mut total_scale_change = 0.0;
        let mut total_angle_change = 0.0;
        let mut valid_zoom_samples = 0;
        let mut valid_rotate_samples = 0;

        for (px, py, cx, cy) in matched_pairs {
            // Vector from PREVIOUS centroid to PREVIOUS point
            let vec_prev_x = px - prev_center_x;
            let vec_prev_y = py - prev_center_y;

            // Vector from CURRENT centroid to CURRENT point
            // Using centroids isolates rotation/zoom from pan
            let vec_curr_x = cx - curr_center_x;
            let vec_curr_y = cy - curr_center_y;

            // Distance (Zoom)
            let dist_prev = (vec_prev_x.powi(2) + vec_prev_y.powi(2)).sqrt();
            let dist_curr = (vec_curr_x.powi(2) + vec_curr_y.powi(2)).sqrt();

            // Ignore points extremely close to centroid to avoid division by zero instability
            // 200 units is about 0.3% of screen width (65535)
            if dist_prev > 200.0 {
                total_scale_change += (dist_curr / dist_prev) - 1.0;
                valid_zoom_samples += 1;
            }

            // Angle (Rotate)
            // Only calculate angle if we are far enough from center to get a stable angle
            if dist_prev > 200.0 && dist_curr > 200.0 {
                let angle_prev = vec_prev_y.atan2(vec_prev_x);
                let angle_curr = vec_curr_y.atan2(vec_curr_x);

                let mut angle_delta = angle_curr - angle_prev;
                // Normalize angle to -PI..PI
                while angle_delta > PI {
                    angle_delta -= 2.0 * PI;
                }
                while angle_delta < -PI {
                    angle_delta += 2.0 * PI;
                }

                total_angle_change += angle_delta;
                valid_rotate_samples += 1;
            }
        }

        let avg_scale = if valid_zoom_samples > 0 {
            total_scale_change / valid_zoom_samples as f32
        } else {
            0.0
        };

        let avg_rotate = if valid_rotate_samples > 0 {
            total_angle_change / valid_rotate_samples as f32
        } else {
            0.0
        };

        // Update previous state for next frame
        self.update_previous(slots);

        // --- DEADZONES & THRESHOLDS ---
        let mut final_pan_x = delta_x;
        let mut final_pan_y = delta_y;
        let mut final_zoom = avg_scale;
        let mut final_rotate = avg_rotate;

        // Deadzones (minimum movement to trigger event)
        // Pan: 0.1% of screen
        if final_pan_x.abs() < 0.001 {
            final_pan_x = 0.0;
        }
        if final_pan_y.abs() < 0.001 {
            final_pan_y = 0.0;
        }

        // Zoom: 0.5% scale change
        if final_zoom.abs() < 0.005 {
            final_zoom = 0.0;
        }

        // Rotate: ~0.5 degrees (0.008 radians)
        if final_rotate.abs() < 0.008 {
            final_rotate = 0.0;
        }

        Some(GestureDelta {
            pan_x: final_pan_x,
            pan_y: final_pan_y,
            zoom: final_zoom,
            rotate: final_rotate,
        })
    }

    fn update_previous(&mut self, slots: &[TouchSlot]) {
        for (i, slot) in slots.iter().enumerate() {
            if slot.active {
                self.previous_points[i] = Some((slot.tracking_id, slot.x as f32, slot.y as f32));
            } else {
                self.previous_points[i] = None;
            }
        }
    }
}
