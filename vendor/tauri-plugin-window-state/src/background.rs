// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::{writer::Writer, PluginState, Result, StateFlags, WindowExtInternal, WindowStateCache};
use std::{path::PathBuf, sync::mpsc, thread::ThreadId, time::Duration};
use tauri::{AppHandle, Manager, Runtime};

pub(crate) struct Background {
    pub(crate) writer: Writer,
    pub(crate) shutdown_timeout: Duration,
    main_thread: ThreadId,
}

impl Background {
    pub(crate) fn new(path: PathBuf, shutdown_timeout: Duration) -> Result<Self> {
        Ok(Self {
            writer: Writer::new(path)?,
            shutdown_timeout,
            // Tauri runs plugin setup on the event-loop thread.
            main_thread: std::thread::current().id(),
        })
    }
}

pub(crate) fn needs_dispatch<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<PluginState>()
        .background
        .as_ref()
        .is_some_and(|background| background.main_thread != std::thread::current().id())
}

/// No cache/queue lock may be held while an off-main caller waits for capture.
pub(crate) fn on_main<R, F>(app: &AppHandle<R>, capture: F) -> Result<()>
where
    R: Runtime,
    F: FnOnce(AppHandle<R>) -> Result<()> + Send + 'static,
{
    let handle = app.clone();
    dispatch_capture(
        !needs_dispatch(app),
        |task| app.run_on_main_thread(task),
        move || capture(handle),
    )
}

fn dispatch_capture<F, D>(on_main: bool, dispatch: D, capture: F) -> Result<()>
where
    F: FnOnce() -> Result<()> + Send + 'static,
    D: FnOnce(Box<dyn FnOnce() + Send>) -> tauri::Result<()>,
{
    if on_main {
        return capture();
    }
    let (send, receive) = mpsc::channel();
    dispatch(Box::new(move || {
        let _ = send.send(capture());
    }))?;
    receive.recv().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "window-state capture cancelled",
        )
    })?
}

pub(crate) fn capture<R: Runtime>(app: &AppHandle<R>, flags: StateFlags) -> Result<()> {
    let plugin = app.state::<PluginState>();
    let cache = app.state::<WindowStateCache>();
    let mut snapshot = cache.0.lock().unwrap().clone();
    let windows = app.webview_windows();
    for (label, state) in &mut snapshot {
        let window = if let Some(map) = &plugin.map_label {
            windows
                .iter()
                .find_map(|(key, window)| (map(key) == label).then_some(window))
        } else {
            windows.get(label)
        };
        if let Some(window) = window {
            window.update_state(state, flags)?;
        }
    }
    *cache.0.lock().unwrap() = snapshot.clone();
    // Captures and submissions are serialized on the main thread. The worker
    // owns only this snapshot and its path, never the cache or a window handle.
    plugin
        .background
        .as_ref()
        .unwrap()
        .writer
        .enqueue(snapshot)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_thread_capture_is_inline_and_propagates_errors() {
        let result = dispatch_capture(
            true,
            |_| panic!("main-thread capture must not enqueue and wait on itself"),
            || Err(std::io::Error::other("capture failed").into()),
        );
        assert!(result.unwrap_err().to_string().contains("capture failed"));
    }

    #[test]
    fn off_main_capture_completes_on_dispatch_thread_before_returning() {
        let main_thread = std::thread::current().id();
        let (send, receive) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let (finished, completion) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            let result = dispatch_capture(
                false,
                |task| {
                    send.send(task).unwrap();
                    Ok(())
                },
                move || {
                    assert_eq!(std::thread::current().id(), main_thread);
                    Err(std::io::Error::other("native capture error").into())
                },
            );
            finished.send(result).unwrap();
        });
        let task = receive.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(completion.try_recv().is_err());
        task();
        assert!(completion
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("native capture error"));
        caller.join().unwrap();
    }

    #[test]
    fn cancelled_capture_reports_error_instead_of_hanging() {
        let result = dispatch_capture(false, |_| Ok(()), || Ok(()));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("capture cancelled"));
    }

    #[test]
    fn failed_dispatch_does_not_wait_for_a_capture() {
        let result = dispatch_capture(
            false,
            |_| Err(tauri::Error::Io(std::io::Error::other("event loop closed"))),
            || panic!("capture must not run after failed dispatch"),
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("event loop closed"));
    }
}
