//! XDG Desktop Portal screen selection.

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use thiserror::Error;
use tracing::info;
use std::sync::Arc;
use std::any::Any;

/// Errors that can occur during portal operations.
#[derive(Debug, Error)]
pub enum PortalError {
    #[error("portal error: {0}")]
    Portal(#[from] ashpd::Error),

    #[error("no streams available")]
    NoStreams,

    #[error("portal cancelled by user")]
    Cancelled,
}

/// Information about a selected monitor/screen.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// PipeWire node ID for the stream.
    pub node_id: u32,
    /// Width of the captured area.
    pub width: u32,
    /// Height of the captured area.
    pub height: u32,
    /// Restore token for persistent selection.
    pub restore_token: Option<String>,
    /// Keep the portal session alive.
    pub _session: Arc<dyn Any + Send + Sync>,
}

/// Opens the XDG Desktop Portal screen selection dialog.
///
/// Returns information about the selected monitor if successful.
pub async fn select_monitor(restore_token: Option<&str>) -> Result<MonitorInfo, PortalError> {
    info!("Opening screen selection portal");

    info!("Creating screencast proxy...");
    let proxy = Screencast::new().await?;
    info!("Screencast proxy created.");

    info!("Creating session...");
    let session = proxy.create_session().await?;
    info!("Session created.");

    info!("Selecting sources...");
    // Select sources (monitor only, with cursor)
    proxy
        .select_sources(
            &session,
            CursorMode::Embedded,
            SourceType::Monitor.into(),
            false, // Don't allow multiple sources
            restore_token,
            PersistMode::DoNot,
        )
        .await?;
    info!("Sources selected.");

    info!("Starting screencast...");
    // Start the screencast
    let response = proxy.start(&session, None).await?.response()?;
    info!("Screencast started.");

    let streams = response.streams();
    if streams.is_empty() {
        return Err(PortalError::NoStreams);
    }

    let stream = &streams[0];
    let (width, height) = stream.size().unwrap_or((1920, 1080));

    info!(
        "Screen selected: node_id={}, size={}x{}",
        stream.pipe_wire_node_id(),
        width,
        height
    );

    Ok(MonitorInfo {
        node_id: stream.pipe_wire_node_id(),
        width: width as u32,
        height: height as u32,
        restore_token: response.restore_token().map(|s| s.to_string()),
        _session: Arc::new(session),
    })
}