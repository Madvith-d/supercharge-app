// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::WindowState;
use std::{
    collections::HashMap,
    io::{self, Write},
    path::Path,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub(crate) type Snapshot = HashMap<String, WindowState>;
const RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const WARNING_INTERVAL: Duration = Duration::from_secs(60);

fn next_retry_delay(previous: Duration) -> Duration {
    previous.saturating_mul(2).min(MAX_RETRY_DELAY)
}

#[derive(Default)]
struct Queue {
    pending: Option<Snapshot>,
    deadline: Option<Instant>,
    finished: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}

/// One in-flight write and one replaceable pending snapshot; never queues native handles.
pub(crate) struct Writer {
    shared: Arc<Shared>,
}

impl Writer {
    pub(crate) fn new(path: std::path::PathBuf) -> io::Result<Self> {
        Self::start(move |snapshot| atomic_write(&path, snapshot))
    }

    fn start<F>(mut write: F) -> io::Result<Self>
    where
        F: FnMut(&Snapshot) -> io::Result<()> + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("window-state-writer".into())
            .spawn(move || {
                let mut retry_delay = RETRY_DELAY;
                let mut next_warning = Instant::now();
                let mut queue = worker.queue.lock().unwrap();
                loop {
                    if queue.deadline.is_some_and(|end| Instant::now() >= end)
                        || (queue.deadline.is_some() && queue.pending.is_none())
                    {
                        queue.finished = true;
                        worker.changed.notify_all();
                        return;
                    }
                    let Some(snapshot) = queue.pending.take() else {
                        queue = worker.changed.wait(queue).unwrap();
                        continue;
                    };
                    drop(queue);
                    let result = write(&snapshot);
                    if let Err(error) = &result {
                        if Instant::now() >= next_warning {
                            log::warn!("window-state background save failed; will retry: {error}");
                            next_warning = Instant::now() + WARNING_INTERVAL;
                        }
                    } else {
                        retry_delay = RETRY_DELAY;
                        next_warning = Instant::now();
                    }
                    queue = worker.queue.lock().unwrap();
                    if result.is_err() {
                        // A newer capture always supersedes a failed older snapshot.
                        let superseded = queue.pending.is_some();
                        if !superseded {
                            queue.pending = Some(snapshot);
                        }
                        let delay = queue.deadline.map_or(retry_delay, |end| {
                            RETRY_DELAY.min(end.saturating_duration_since(Instant::now()))
                        });
                        retry_delay = next_retry_delay(retry_delay);
                        if !superseded {
                            queue = worker.changed.wait_timeout(queue, delay).unwrap().0;
                        }
                    }
                }
            })?;
        Ok(Self { shared })
    }

    pub(crate) fn enqueue(&self, snapshot: Snapshot) -> io::Result<()> {
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.deadline.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "window-state writer stopped",
            ));
        }
        queue.pending = Some(snapshot);
        self.shared.changed.notify_one();
        Ok(())
    }

    /// Stops accepting snapshots and waits at most `timeout`, including retries.
    /// An already-running filesystem operation cannot be cancelled or joined safely.
    pub(crate) fn shutdown(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut queue = self.shared.queue.lock().unwrap();
        queue.deadline = Some(queue.deadline.map_or(end, |old| old.min(end)));
        self.shared.changed.notify_all();
        while !queue.finished {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            queue = self
                .shared
                .changed
                .wait_timeout(queue, remaining)
                .unwrap()
                .0;
        }
        queue.pending.is_none()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.deadline = Some(Instant::now());
        self.shared.changed.notify_all();
    }
}

fn atomic_write(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing state directory"))?;
    std::fs::create_dir_all(parent)?;
    // Serialize and sync a same-directory temporary file before atomically replacing
    // the destination. Failed serialization/write/rename leaves the old file intact.
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
