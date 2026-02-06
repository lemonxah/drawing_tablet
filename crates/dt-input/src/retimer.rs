//! Event retiming buffer for smooth input delivery.
//!
//! This module buffers incoming input events and emits them with proper
//! timing based on their original timestamps, preventing bursts from
//! network jitter that can confuse applications like Krita.

use dt_protocol::InputPacket;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::debug;

/// Maximum events to buffer (prevents unbounded memory usage).
const MAX_BUFFER_SIZE: usize = 64;

/// Maximum delay to wait for an event (prevents excessive lag).
const MAX_DELAY_MS: u64 = 50;

/// Minimum time between events (prevents too-fast emission).
const MIN_INTERVAL_US: u64 = 1000; // 1ms = 1000Hz max

/// Buffers input events and emits them with proper timing.
pub struct EventRetimer {
    /// Buffered events with their target emission time.
    buffer: VecDeque<(Instant, InputPacket)>,
    /// Timestamp offset: maps client timestamps to local Instant.
    /// Set on first event to sync clocks.
    clock_offset: Option<(u64, Instant)>, // (client_timestamp_us, local_instant)
    /// Last emission time to enforce minimum interval.
    last_emit: Option<Instant>,
}

impl EventRetimer {
    pub fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            clock_offset: None,
            last_emit: None,
        }
    }

    /// Reset the retimer state (e.g., on disconnect).
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.clock_offset = None;
        self.last_emit = None;
    }

    /// Add an event to the buffer.
    /// Returns the event immediately if retiming is not beneficial.
    pub fn push(&mut self, packet: InputPacket) {
        let now = Instant::now();

        // Initialize clock sync on first event
        if self.clock_offset.is_none() {
            self.clock_offset = Some((packet.timestamp, now));
            debug!(
                "Event retimer: clock sync initialized, client_ts={}",
                packet.timestamp
            );
        }

        // Calculate when this event should be emitted
        let target_time = self.client_ts_to_instant(packet.timestamp, now);

        // Don't buffer events too far in the future
        let max_future = now + Duration::from_millis(MAX_DELAY_MS);
        let target_time = target_time.min(max_future);

        // Add to buffer
        if self.buffer.len() >= MAX_BUFFER_SIZE {
            // Drop oldest event if buffer is full
            self.buffer.pop_front();
            debug!("Event retimer: buffer full, dropped oldest event");
        }
        self.buffer.push_back((target_time, packet));
    }

    /// Get the next event that's ready to be emitted, if any.
    /// Returns None if no events are ready yet.
    pub fn pop_ready(&mut self) -> Option<InputPacket> {
        let now = Instant::now();

        // Check minimum interval
        if let Some(last) = self.last_emit {
            if now.duration_since(last) < Duration::from_micros(MIN_INTERVAL_US) {
                return None;
            }
        }

        // Check if front event is ready
        if let Some((target_time, _)) = self.buffer.front() {
            if now >= *target_time {
                self.last_emit = Some(now);
                return self.buffer.pop_front().map(|(_, packet)| packet);
            }
        }

        None
    }

    /// Get all events that are ready to be emitted.
    pub fn drain_ready(&mut self) -> Vec<InputPacket> {
        let mut ready = Vec::new();
        while let Some(packet) = self.pop_ready() {
            ready.push(packet);
        }
        ready
    }

    /// Time until the next event is ready, or None if buffer is empty.
    pub fn time_until_next(&self) -> Option<Duration> {
        let now = Instant::now();

        self.buffer.front().map(|(target_time, _)| {
            if *target_time > now {
                *target_time - now
            } else {
                Duration::ZERO
            }
        })
    }

    /// Check if there are buffered events.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Convert a client timestamp (microseconds, monotonic) to a local Instant.
    fn client_ts_to_instant(&self, client_ts: u64, now: Instant) -> Instant {
        if let Some((base_client_ts, base_instant)) = self.clock_offset {
            // Calculate delta from first event
            if client_ts >= base_client_ts {
                let delta_us = client_ts - base_client_ts;
                base_instant + Duration::from_micros(delta_us)
            } else {
                // Client timestamp wrapped or went backwards, use now
                now
            }
        } else {
            now
        }
    }
}

impl Default for EventRetimer {
    fn default() -> Self {
        Self::new()
    }
}
