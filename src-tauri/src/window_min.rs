//! Main-window OS min size vs tiling.
//!
//! Config `minWidth` / `minHeight` are a comfort floor. Aero Snap, Win+arrows,
//! and tiling WMs refuse to size below `WM_GETMINMAXINFO` / the Tauri min, so a
//! 900px floor on a 1440-wide work area becomes ~2/3 of the screen. Cap the OS
//! min to half the current monitor work area (taskbar excluded). Large
//! displays keep 900×600; moving onto a bigger screen restores that floor.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

use tauri::{AppHandle, LogicalSize, Manager, Monitor};

/// Comfort fallback when the window config omits min size.
const FALLBACK_MIN_W: f64 = 900.0;
const FALLBACK_MIN_H: f64 = 600.0;

/// Cap one axis: never larger than half of `work` (floored so min ≤ true half).
pub fn snap_friendly_min(comfort: f64, work: f64) -> f64 {
    let comfort = if comfort.is_finite() && comfort > 0.0 {
        comfort
    } else {
        1.0
    };
    if !work.is_finite() || work <= 0.0 {
        return comfort;
    }
    let half = (work / 2.0).floor();
    if half <= 0.0 {
        return comfort;
    }
    comfort.min(half)
}

pub fn snap_friendly_min_size(
    comfort_w: f64,
    comfort_h: f64,
    work_w: f64,
    work_h: f64,
) -> (f64, f64) {
    (
        snap_friendly_min(comfort_w, work_w),
        snap_friendly_min(comfort_h, work_h),
    )
}

/// Logical work-area size (taskbar excluded), matching OS snap.
pub fn work_logical(monitor: &Monitor) -> (f64, f64) {
    let scale = monitor.scale_factor().max(0.1);
    let s = monitor.work_area().size;
    (f64::from(s.width) / scale, f64::from(s.height) / scale)
}

pub fn cap_for_monitor(comfort_w: f64, comfort_h: f64, monitor: Option<&Monitor>) -> (f64, f64) {
    match monitor {
        Some(m) => {
            let (ww, wh) = work_logical(m);
            snap_friendly_min_size(comfort_w, comfort_h, ww, wh)
        }
        None => (
            snap_friendly_min(comfort_w, f64::INFINITY),
            snap_friendly_min(comfort_h, f64::INFINITY),
        ),
    }
}

fn comfort_from_config(app: &AppHandle) -> (f64, f64) {
    let w = app.config().app.windows.iter().find(|w| w.label == "main");
    (
        w.and_then(|c| c.min_width)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(FALLBACK_MIN_W),
        w.and_then(|c| c.min_height)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(FALLBACK_MIN_H),
    )
}

/// Last committed (min_w, min_h, scale×100). Skip tao `set_min_size` when
/// unchanged — it always `set_inner_size`s, which grows undecorated-shadow
/// windows (outer−inner added twice) and clears WS_MAXIMIZE.
static LAST_MIN: Mutex<Option<(u32, u32, u32)>> = Mutex::new(None);

pub fn should_commit_min(
    maximized: bool,
    pointer_down: bool,
    last: Option<(u32, u32, u32)>,
    next: (u32, u32, u32),
) -> bool {
    if maximized || pointer_down {
        return false;
    }
    last != Some(next)
}

fn min_cache_key(min_w: f64, min_h: f64, scale: f64) -> (u32, u32, u32) {
    (
        min_w.round() as u32,
        min_h.round() as u32,
        (scale.max(0.1) * 100.0).round() as u32,
    )
}

const SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(150);
static UPDATE_GENERATION: AtomicU64 = AtomicU64::new(0);
static UPDATE_PENDING: AtomicBool = AtomicBool::new(false);

/// Keep one pending update and wait until native geometry events settle.
pub fn schedule_main(app: &AppHandle) {
    UPDATE_GENERATION.fetch_add(1, Ordering::SeqCst);
    if UPDATE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let generation = UPDATE_GENERATION.load(Ordering::SeqCst);
            tokio::time::sleep(SETTLE_DELAY).await;
            if generation != UPDATE_GENERATION.load(Ordering::SeqCst) {
                continue;
            }
            let target = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                UPDATE_PENDING.store(false, Ordering::SeqCst);
                if generation != UPDATE_GENERATION.load(Ordering::SeqCst) || apply_main(&target) {
                    schedule_main(&target);
                }
            }) {
                UPDATE_PENDING.store(false, Ordering::SeqCst);
                tracing::warn!(%error, "could not dispatch minimum window size update");
            }
            break;
        }
    });
}

fn can_adjust_geometry(minimized: bool, maximized: bool, fullscreen: bool, visible: bool) -> bool {
    !minimized && !maximized && !fullscreen && visible
}

/// Record constraints already applied by successful window creation.
pub fn remember_configured_min(min_w: f64, min_h: f64, scale: f64) {
    *LAST_MIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(min_cache_key(min_w, min_h, scale));
}

fn after_pointer_release(pointer_down: bool, apply: impl FnOnce()) -> bool {
    if pointer_down {
        return true;
    }
    apply();
    false
}

/// Returns true when a held pointer requires another deferred attempt.
fn apply_main(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    if !can_adjust_geometry(
        window.is_minimized().unwrap_or(true),
        window.is_maximized().unwrap_or(true),
        window.is_fullscreen().unwrap_or(true),
        window.is_visible().unwrap_or(false),
    ) {
        return false;
    }
    let pointer_down = {
        #[cfg(windows)]
        {
            crate::win_shell::primary_mouse_button_down()
        }
        #[cfg(not(windows))]
        {
            false
        }
    };
    after_pointer_release(pointer_down, || {
        let (cw, ch) = comfort_from_config(app);
        let Ok(Some(monitor)) = window.current_monitor() else {
            return;
        };
        let (min_w, min_h) = cap_for_monitor(cw, ch, Some(&monitor));
        let scale = window.scale_factor().unwrap_or(1.0);
        let next = min_cache_key(min_w, min_h, scale);
        let mut last = LAST_MIN.lock().unwrap_or_else(|e| e.into_inner());
        if !should_commit_min(false, false, *last, next) {
            return;
        }
        // Reserve before the setter, which can synchronously emit another move.
        *last = Some(next);
        drop(last);
        if let Err(error) = window.set_min_size(Some(LogicalSize::new(min_w, min_h))) {
            let mut last = LAST_MIN.lock().unwrap_or_else(|e| e.into_inner());
            if *last == Some(next) {
                *last = None;
            }
            tracing::warn!(%error, "minimum window size update failed");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        after_pointer_release, can_adjust_geometry, min_cache_key, should_commit_min,
        snap_friendly_min, snap_friendly_min_size,
    };

    #[test]
    fn paused_drag_retries_until_release_without_another_move() {
        let mut applications = 0;
        let mut pending = true;
        for pointer_down in [true, true, false] {
            assert!(pending);
            pending = after_pointer_release(pointer_down, || applications += 1);
        }
        assert!(!pending);
        assert_eq!(applications, 1);
    }

    #[test]
    fn configured_minimum_skips_initial_visible_resize_but_tracks_monitor_change() {
        let configured = min_cache_key(900.0, 540.0, 1.0);
        assert!(!should_commit_min(
            false,
            false,
            Some(configured),
            configured
        ));
        assert!(should_commit_min(
            false,
            false,
            Some(configured),
            min_cache_key(640.0, 340.0, 1.5),
        ));
    }

    #[test]
    fn only_visible_restored_windows_allow_geometry_adjustment() {
        for minimized in [false, true] {
            for maximized in [false, true] {
                for fullscreen in [false, true] {
                    for visible in [false, true] {
                        assert_eq!(
                            can_adjust_geometry(minimized, maximized, fullscreen, visible),
                            !minimized && !maximized && !fullscreen && visible
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn half_of_1440_beats_comfort_900() {
        // 1440×900 @ 100%, work 1440×852. Win+Right half = 720.
        assert_eq!(snap_friendly_min(900.0, 1440.0), 720.0);
        assert_eq!(snap_friendly_min(600.0, 852.0), 426.0);
    }

    #[test]
    fn large_display_keeps_comfort() {
        assert_eq!(snap_friendly_min(900.0, 1920.0), 900.0);
        assert_eq!(snap_friendly_min(900.0, 2560.0), 900.0);
        // 1080 work height: half is 540, so comfort 600 cannot be kept.
        assert_eq!(snap_friendly_min(600.0, 1080.0), 540.0);
        // 1200+ work height keeps 600.
        assert_eq!(snap_friendly_min(600.0, 1200.0), 600.0);
    }

    #[test]
    fn scaled_1080p_and_1366() {
        assert_eq!(snap_friendly_min(900.0, 1280.0), 640.0); // 1920@150%
        assert_eq!(snap_friendly_min(900.0, 1366.0), 683.0);
        assert_eq!(snap_friendly_min(900.0, 1536.0), 768.0); // 1920@125%
    }

    #[test]
    fn floor_keeps_min_at_or_below_half() {
        assert_eq!(snap_friendly_min(900.0, 1001.0), 500.0);
    }

    #[test]
    fn missing_work_keeps_comfort() {
        assert_eq!(snap_friendly_min(900.0, f64::INFINITY), 900.0);
        assert_eq!(snap_friendly_min(900.0, 0.0), 900.0);
        assert_eq!(snap_friendly_min(900.0, f64::NAN), 900.0);
    }

    #[test]
    fn size_caps_both_axes() {
        assert_eq!(
            snap_friendly_min_size(900.0, 600.0, 1440.0, 852.0),
            (720.0, 426.0)
        );
        assert_eq!(
            snap_friendly_min_size(900.0, 600.0, 1920.0, 1080.0),
            (900.0, 540.0)
        );
        assert_eq!(
            snap_friendly_min_size(900.0, 600.0, 1920.0, 1200.0),
            (900.0, 600.0)
        );
    }

    #[test]
    fn commit_min_skips_drag_maximize_and_repeats() {
        let key = (900, 600, 200);
        assert!(!should_commit_min(true, false, None, key));
        assert!(!should_commit_min(false, true, None, key));
        assert!(!should_commit_min(false, false, Some(key), key));
        assert!(should_commit_min(false, false, None, key));
        assert!(should_commit_min(false, false, Some(key), (720, 600, 200)));
        assert!(should_commit_min(false, false, Some(key), (900, 600, 100)));
    }
}
