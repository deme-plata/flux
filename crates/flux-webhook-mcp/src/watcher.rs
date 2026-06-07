//! Aether file watcher — monitors file changes and emits events.
//!
//! Uses the `notify` crate to watch directories for file changes,
//! then emits WebhookEvents on the event bus. Smarter than polling:
//! - Uses inotify (Linux) / FSEvents (macOS) for instant notification
//! - Debounces rapid changes (batches within 100ms window)
//! - Only emits for supported file types
//! - Rate-limited to avoid storming the event bus

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::types::*;

/// Debounce window for file changes (milliseconds).
const DEBOUNCE_MS: u64 = 100;

/// File extensions that trigger events.
const WATCHED_EXTENSIONS: &[&str] = &["rs", "toml", "md", "json", "yaml", "ts", "tsx", "js", "css"];

/// The aether file watcher.
pub struct AetherWatcher {
    watch_dirs: Vec<String>,
    event_tx: broadcast::Sender<WebhookEvent>,
}

impl AetherWatcher {
    /// Create a new file watcher.
    pub fn new(watch_dirs: Vec<String>, event_tx: broadcast::Sender<WebhookEvent>) -> Self {
        Self { watch_dirs, event_tx }
    }

    /// Run the file watcher.
    pub async fn run(&self) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);

        // Configure notify watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.try_send(event);
                }
            },
            Config::default(),
        )?;

        // Add watch directories
        for dir in &self.watch_dirs {
            let path = PathBuf::from(dir);
            if path.exists() {
                watcher.watch(&path, RecursiveMode::Recursive)?;
                info!("👀 Watching: {}", dir);
            } else {
                warn!("Watch dir does not exist: {}", dir);
                // Create it
                std::fs::create_dir_all(&path)?;
                watcher.watch(&path, RecursiveMode::Recursive)?;
                info!("👀 Created and watching: {}", dir);
            }
        }

        // Debounce state
        let mut pending: HashMap<PathBuf, FileChangeEvent> = HashMap::new();
        let mut last_flush = Instant::now();

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    for path in event.paths {
                        // Check extension
                        let ext = path.extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");
                        if !WATCHED_EXTENSIONS.contains(&ext) {
                            continue;
                        }

                        let kind = match event.kind {
                            EventKind::Create(_) => FileChangeKind::Created,
                            EventKind::Modify(_) => FileChangeKind::Modified,
                            EventKind::Remove(_) => FileChangeKind::Deleted,
                            _ => continue,
                        };

                        let change = FileChangeEvent {
                            path: path.to_string_lossy().to_string(),
                            kind,
                            file_cid: None,
                            file_size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                        };

                        pending.insert(path, change);
                    }

                    // Flush debounced events
                    if last_flush.elapsed() >= Duration::from_millis(DEBOUNCE_MS) {
                        self.flush_events(&mut pending).await;
                        last_flush = Instant::now();
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)) => {
                    if !pending.is_empty() {
                        self.flush_events(&mut pending).await;
                        last_flush = Instant::now();
                    }
                }
            }
        }
    }

    /// Flush pending file change events to the event bus.
    async fn flush_events(&self, pending: &mut HashMap<PathBuf, FileChangeEvent>) {
        let changes: Vec<FileChangeEvent> = pending.drain().map(|(_, v)| v).collect();
        if changes.is_empty() { return; }

        let event = WebhookEvent::new(
            crate::types::event_types::FILE_EDITED,
            "watcher",
            serde_json::to_value(&changes).unwrap_or_default(),
        )
        .with_fluxfood()
        .with_search();

        if self.event_tx.send(event).is_err() {
            warn!("File watcher event lost (no subscribers)");
        } else {
            info!("📁 Watcher: {} file changes dispatched", changes.len());
        }
    }
}
