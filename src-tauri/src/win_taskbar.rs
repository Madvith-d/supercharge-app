//! Explorer taskbar COM calls run on one worker, never on the window event thread.

#![cfg(windows)]

#[path = "win_taskbar_queue.rs"]
mod queue;

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use queue::{Mailbox, Request, CAPACITY, MAX_GENERATION};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetPropW, GetWindowLongW, GetWindowThreadProcessId, IsWindow,
    SetPropW, GWL_EXSTYLE, HICON, ICONINFO, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

static MAILBOX: Mailbox = Mailbox::new();
static EVENT_THREAD: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicUsize = AtomicUsize::new(1);
static WORKER: OnceLock<Option<std::thread::Thread>> = OnceLock::new();
const LIFETIME_PROP: PCWSTR = windows::core::w!("GrokTaskbarLifetime");
const SLOW_CALL: Duration = Duration::from_millis(100);

/// Enqueue from the window event thread, after applying native styles/icons.
/// Main-window presence must include `Some(win_taskbar_overlay::last_count())`.
/// Tool/pet windows must pass `None`.
pub fn set_tab(hwnd: HWND, present: bool, overlay_count: Option<u32>) {
    let Some(index) = slot_for(hwnd) else {
        return;
    };
    MAILBOX.set_tab(index, present, overlay_count);
    wake_worker();
}

/// Enqueue from the window event thread; does not add or remove a taskbar tab.
pub fn set_overlay(hwnd: HWND, count: u32) {
    let Some(index) = slot_for(hwnd) else {
        return;
    };
    MAILBOX.set_overlay(index, count);
    wake_worker();
}

fn slot_for(hwnd: HWND) -> Option<usize> {
    unsafe {
        let mut process = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut process));
        let current = GetCurrentThreadId();
        if thread != current || process != std::process::id() {
            tracing::warn!("win_taskbar: enqueue must run on the owning window event thread");
            return None;
        }
        let owner = EVENT_THREAD
            .compare_exchange(0, current, Ordering::Relaxed, Ordering::Relaxed)
            .unwrap_or_else(|owner| owner);
        if owner != 0 && owner != current {
            tracing::warn!("win_taskbar: windows must share one event thread");
            return None;
        }
        let cookie = GetPropW(hwnd, LIFETIME_PROP).0 as usize;
        let mut vacant = None;
        for index in 0..CAPACITY {
            match MAILBOX.current(index) {
                Some(request)
                    if request.hwnd == hwnd.0 as usize && request.generation() == cookie =>
                {
                    return Some(index);
                }
                Some(request) if live_window(request) => {}
                _ => {
                    vacant.get_or_insert(index);
                }
            }
        }
        let Some(index) = vacant else {
            tracing::warn!("win_taskbar: all {CAPACITY} window mailboxes are occupied");
            return None;
        };
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        if generation > MAX_GENERATION {
            tracing::error!("win_taskbar: window lifetime counter exhausted");
            return None;
        }
        if let Err(error) = SetPropW(hwnd, LIFETIME_PROP, Some(HANDLE(generation as *mut _))) {
            tracing::warn!(%error, "win_taskbar: could not mark window lifetime");
            return None;
        }
        // Window properties disappear on destruction, even if Windows reuses the HWND.
        MAILBOX.install(index, hwnd.0 as usize, generation, app_window(hwnd));
        Some(index)
    }
}

fn wake_worker() {
    let worker = WORKER.get_or_init(|| {
        match std::thread::Builder::new()
            .name("windows-taskbar".into())
            .spawn(worker_loop)
        {
            Ok(handle) => Some(handle.thread().clone()),
            Err(error) => {
                tracing::error!(%error, "win_taskbar: could not start worker");
                None
            }
        }
    });
    if let Some(worker) = worker {
        worker.unpark();
    }
}

fn worker_loop() {
    // Activate/use the taskbar proxy in MTA; an idle worker has no STA message pump.
    if timed("CoInitializeEx", || unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()
    })
    .is_err()
    {
        return;
    }
    let _com = ComApartment;
    let mut taskbar = None;
    loop {
        for index in 0..CAPACITY {
            let Some(request) = MAILBOX.take(index) else {
                continue;
            };
            if latest(request).is_none() {
                continue;
            }
            if taskbar.is_none() {
                taskbar = new_taskbar().ok();
            }
            if let Some(shell) = taskbar.as_ref() {
                if apply(shell, request).is_err() {
                    // Explorer may have restarted; reconnect on the next request.
                    taskbar = None;
                }
            }
        }
        // Unpark retains a permit if an enqueue raced the scan above.
        std::thread::park();
    }
}

struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn new_taskbar() -> Result<ITaskbarList3> {
    let taskbar: ITaskbarList3 = timed("CoCreateInstance", || unsafe {
        CoCreateInstance(&TaskbarList, None, CLSCTX_SERVER)
    })?;
    timed("HrInit", || unsafe { taskbar.HrInit() })?;
    Ok(taskbar)
}

fn timed<T>(operation: &'static str, call: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let result = call();
    let elapsed = started.elapsed();
    if elapsed >= SLOW_CALL {
        tracing::warn!(
            operation,
            elapsed_ms = elapsed.as_millis() as u64,
            "win_taskbar: slow native taskbar call"
        );
    }
    if let Err(error) = &result {
        tracing::warn!(operation, %error, "win_taskbar: native call failed");
    }
    result
}

fn live_window(request: Request) -> bool {
    let hwnd = HWND(request.hwnd as *mut _);
    unsafe {
        let mut process = 0;
        IsWindow(Some(hwnd)).as_bool()
            && GetWindowThreadProcessId(hwnd, Some(&mut process)) != 0
            && process == std::process::id()
            && GetPropW(hwnd, LIFETIME_PROP).0 as usize == request.generation()
    }
}

fn latest(request: Request) -> Option<Request> {
    let current = MAILBOX.current(request.index)?;
    (current.generation() == request.generation()
        && current.hwnd == request.hwnd
        && current.present() == request.present()
        && live_window(current))
    .then_some(current)
}

fn app_window(hwnd: HWND) -> bool {
    let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    style & WS_EX_TOOLWINDOW.0 == 0 && style & WS_EX_APPWINDOW.0 != 0
}

fn apply(taskbar: &ITaskbarList3, request: Request) -> Result<()> {
    let hwnd = HWND(request.hwnd as *mut _);
    if latest(request).is_none() {
        return Ok(());
    }
    if request.refresh() {
        if request.present() && !app_window(hwnd) {
            return Ok(());
        }
        let deleted = timed("DeleteTab", || unsafe { taskbar.DeleteTab(hwnd) });
        if !request.present() {
            return deleted;
        }
        // DeleteTab may fail for a tab not yet registered. Still attempt AddTab.
        if latest(request).is_none() || !app_window(hwnd) {
            return Ok(());
        }
        timed("AddTab", || unsafe { taskbar.AddTab(hwnd) })?;
    }
    // AddTab clears the badge. Read the newest count after the COM call returns.
    if let Some(current) = latest(request) {
        if current.present() && app_window(hwnd) {
            if let Some(count) = current.overlay_count() {
                let icon = overlay_icon(count)?;
                if latest(request).is_some() && app_window(hwnd) {
                    timed("SetOverlayIcon", || unsafe {
                        taskbar.SetOverlayIcon(
                            hwnd,
                            icon.as_ref().map_or(HICON::default(), |icon| icon.0),
                            PCWSTR::null(),
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}

struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        let _ = timed("DestroyIcon", || unsafe { DestroyIcon(self.0) });
    }
}

struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !unsafe { DeleteObject(HGDIOBJ(self.0 .0)) }.as_bool() {
            tracing::warn!("win_taskbar: DeleteObject failed for overlay bitmap");
        }
    }
}

fn overlay_icon(count: u32) -> Result<Option<OwnedIcon>> {
    let Some(mut rgba) = crate::win_taskbar_overlay::overlay_rgba(count) else {
        return Ok(None);
    };
    let size = crate::win_taskbar_overlay::SIZE as i32;
    // A top-down 32-bit DIB uses premultiplied BGRA, not the renderer's straight RGBA.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        for channel in 0..3 {
            pixel[channel] = ((u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255) as u8;
        }
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let mut bits = std::ptr::null_mut();
        let color = OwnedBitmap(timed("CreateDIBSection", || {
            CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
        })?);
        std::ptr::copy_nonoverlapping(rgba.as_ptr(), bits.cast(), rgba.len());
        let mask_bits = [0u8; 32]; // 16 rows, each word-aligned.
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        if mask.0.is_null() {
            let error = windows::core::Error::from_win32();
            tracing::warn!(%error, "win_taskbar: CreateBitmap failed");
            return Err(error);
        }
        let mask = OwnedBitmap(mask);
        let icon = timed("CreateIconIndirect", || {
            CreateIconIndirect(&ICONINFO {
                fIcon: true.into(),
                hbmMask: mask.0,
                hbmColor: color.0,
                ..Default::default()
            })
        })?;
        Ok(Some(OwnedIcon(icon)))
    }
}
