//! Timing for native window operations that must not block the event loop.

use std::time::{Duration, Instant};

const SLOW_OPERATION: Duration = Duration::from_millis(250);

pub(crate) struct WindowOperation {
    name: &'static str,
    started: Instant,
}

impl WindowOperation {
    pub(crate) fn start(name: &'static str) -> Self {
        Self {
            name,
            started: Instant::now(),
        }
    }
}

impl Drop for WindowOperation {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        if elapsed >= SLOW_OPERATION {
            tracing::warn!(
                operation = self.name,
                elapsed_ms,
                "slow native window operation"
            );
        } else {
            tracing::debug!(
                operation = self.name,
                elapsed_ms,
                "native window operation finished"
            );
        }
    }
}
