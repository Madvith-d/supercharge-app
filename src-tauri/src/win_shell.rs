//! Windows shell integration for the main workbench window.
//!
//! Frameless (`decorations: false`) + tray `set_skip_taskbar` can leave the HWND
//! in a state where Explorer's **Show Desktop** (taskbar far-right / Win+D)
//! does not treat us as a significant top-level app window when we are alone.
//! With other normal windows open, minimize-all still sweeps us up — matching
//! the reported "alone = no effect; multi-window = works" symptom.
//!
//! This module forces shell-friendly styles, AppUserModelID, and taskbar tab
//! registration so the window participates in Show Desktop consistently.
//!
//! It also forwards Alt-Tab / taskbar activation into the child WebView2 HWND.
//! With Tauri `unstable` (multi-webview), the page is a `WRY_WEBVIEW` child and
//! wry does not subclass the parent to `MoveFocus`, so the window can be
//! foreground while keyboard events never reach JS until a click.

#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use tauri::{Manager, WebviewWindow};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, SetFocus, VK_LBUTTON,
};
use windows::Win32::UI::Shell::{ExtractIconExW, SetCurrentProcessExplicitAppUserModelID};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DrawMenuBar, GetClassNameW, GetPropW, GetWindow, GetWindowLongPtrW,
    GetWindowLongW, GetWindowThreadProcessId, IsChild, IsWindow, IsWindowVisible, IsZoomed,
    RemovePropW, SendMessageW, SetClassLongPtrW, SetMenu, SetPropW, SetWindowLongPtrW,
    SetWindowLongW, SetWindowPos, ShowWindowAsync, GCLP_HICON, GCLP_HICONSM, GWLP_HWNDPARENT,
    GWLP_WNDPROC, GWL_EXSTYLE, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, GW_OWNER, HICON, HWND_NOTOPMOST,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, WA_ACTIVE, WA_CLICKACTIVE, WM_ACTIVATE, WM_NCDESTROY, WM_SETFOCUS,
    WM_SETICON, WNDPROC, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX,
};

/// Call once early in process startup (before or right after creating the main window).
///
/// `id` must be the bundled Tauri `identifier` (release `com.grokapp.desktop`;
/// `pnpm dev` overlay `com.grokapp.desktop.dev`) so Explorer groups this
/// process with the matching shortcuts / toasts, not the other install.
pub fn set_process_app_user_model_id(id: &str) {
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if let Err(e) = SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide.as_ptr())) {
            tracing::warn!("SetCurrentProcessExplicitAppUserModelID: {e}");
        }
    }
}

/// True while the physical left mouse button is down.
///
/// Used by the pet overlay: `startDragging()` often swallows WebView `pointerup`,
/// so the host must notice the button release itself.
pub fn primary_mouse_button_down() -> bool {
    unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0 }
}

/// Cached main HWND for latency-critical caption actions.
///
/// Tauri's window API sends another user event through the tao event loop.
/// Under WebView/resize load that second queue can be delayed or coalesced.
/// `ShowWindowAsync` posts directly to the HWND owner thread and never waits
/// for it, so a caption click cannot be trapped behind unrelated app events.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn post_main_caption_action(action: &str) -> Result<(), String> {
    let raw = MAIN_HWND.load(Ordering::Acquire);
    if raw == 0 {
        return Err("main window handle is not ready".into());
    }
    let hwnd = HWND(raw as *mut std::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            MAIN_HWND.store(0, Ordering::Release);
            return Err("main window handle is no longer valid".into());
        }
        let command = match action {
            "minimize" => SW_MINIMIZE,
            "toggleMaximize" => {
                if IsZoomed(hwnd).as_bool() {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                }
            }
            _ => return Err(format!("unsupported caption action: {action}")),
        };
        if !ShowWindowAsync(hwnd, command).as_bool() {
            return Err("could not post caption action to the main window".into());
        }
    }
    Ok(())
}

/// Dispatch before looking up the HWND: Tauri's off-thread HWND getter can wait.
fn on_window_thread(
    window: &WebviewWindow,
    apply: impl FnOnce(&WebviewWindow, HWND) + Send + 'static,
) {
    let target = window.clone();
    if let Err(error) = window.run_on_main_thread(move || {
        let Ok(hwnd) = target.hwnd() else {
            return;
        };
        let mut process = 0;
        let valid = unsafe {
            IsWindow(Some(hwnd)).as_bool()
                && GetWindowThreadProcessId(hwnd, Some(&mut process)) == GetCurrentThreadId()
                && process == std::process::id()
        };
        if valid {
            apply(&target, hwnd);
        }
    }) {
        tracing::warn!(%error, "win_shell: could not dispatch window operation");
    }
}

/// Desktop-pet overlay: drop the Win32 menu bar (File / Edit / Window / Help).
///
/// Tauri `app.set_menu` attaches the app-wide menu to every window that did not
/// install its own. `SetMenu(NULL)` + `DrawMenuBar` collapses the extra strip
/// even when `decorations(false)` left the muda bar painted.
pub fn strip_overlay_native_menu(window: &WebviewWindow) {
    on_window_thread(window, |_, hwnd| unsafe {
        let _ = SetMenu(hwnd, None);
        let _ = DrawMenuBar(hwnd);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    });
}

/// Ensure the main window is a normal taskbar / Alt-Tab / Show-Desktop participant.
///
/// Safe to call repeatedly (setup, show-from-tray, after skip_taskbar restore).
pub fn ensure_main_window_shell_integration(window: &WebviewWindow) {
    on_window_thread(window, |window, hwnd| {
        if let Some(icon) = window.app_handle().default_window_icon() {
            if let Err(e) = window.set_icon(icon.clone()) {
                tracing::warn!("win_shell: default_window_icon: {e}");
            }
        }
        ensure_hwnd_shell_integration(hwnd);
        attach_hwnd_webview_keyboard_focus(hwnd);
        crate::win_taskbar::set_tab(hwnd, true, Some(crate::win_taskbar_overlay::last_count()));
    });
}

/// Push the exe's first icon onto ICON_BIG / ICON_SMALL before Explorer AddTab.
///
/// Frameless release windows often have ICON_SMALL only. After an NSIS update
/// the icon cache misses and `DeleteTab`+`AddTab` then paints a generic
/// document glyph (#943).
fn apply_exe_window_icons(hwnd: HWND) {
    // Extract once: this path runs on setup, tray restore, and skip_taskbar
    // refresh. ExtractIconExW allocates new HICONs each call.
    static ICONS: std::sync::OnceLock<(isize, isize)> = std::sync::OnceLock::new();
    let (big, small) = *ICONS.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else {
            return (0, 0);
        };
        let wide: Vec<u16> = exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut big = HICON::default();
        let mut small = HICON::default();
        unsafe {
            let n = ExtractIconExW(
                PCWSTR(wide.as_ptr()),
                0,
                Some(std::ptr::from_mut(&mut big)),
                Some(std::ptr::from_mut(&mut small)),
                1,
            );
            if n == 0 {
                tracing::warn!("win_shell: ExtractIconExW returned 0");
                return (0, 0);
            }
            (
                if big.0.is_null() { 0 } else { big.0 as isize },
                if small.0.is_null() {
                    0
                } else {
                    small.0 as isize
                },
            )
        }
    });
    if big == 0 && small == 0 {
        return;
    }
    unsafe {
        // WM_SETICON wParam: ICON_SMALL=0, ICON_BIG=1.
        if big != 0 {
            let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(big)));
            let _ = SetClassLongPtrW(hwnd, GCLP_HICON, big);
        }
        if small != 0 {
            let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(small)));
            let _ = SetClassLongPtrW(hwnd, GCLP_HICONSM, small);
        }
    }
}

/// Overlay / pet: TOOLWINDOW, no APPWINDOW — Explorer must not show a second Grok tab.
pub fn overlay_skip_taskbar_exstyle(ex: u32) -> u32 {
    (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
}

/// Force skip-taskbar on a tool overlay (desktop pet). Must not call
/// [`ensure_main_window_shell_integration`] — that re-applies APPWINDOW.
pub fn set_overlay_skip_taskbar(window: &WebviewWindow) {
    on_window_thread(window, |_, hwnd| {
        unsafe {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let next = overlay_skip_taskbar_exstyle(ex);
            if next != ex {
                SetWindowLongW(hwnd, GWL_EXSTYLE, next as i32);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        crate::win_taskbar::set_tab(hwnd, false, None);
    });
}

/// Apply or clear "live in tray only" extended styles + taskbar tab.
/// Prefer this over bare `set_skip_taskbar` so TOOLWINDOW/APPWINDOW stay consistent.
pub fn set_main_window_skip_taskbar(window: &WebviewWindow, skip: bool) {
    on_window_thread(window, move |_, hwnd| {
        unsafe {
            let mut ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if skip {
                ex |= WS_EX_TOOLWINDOW.0;
                ex &= !WS_EX_APPWINDOW.0;
            } else {
                ex &= !WS_EX_TOOLWINDOW.0;
                ex |= WS_EX_APPWINDOW.0;
            }
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex as i32);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
        if !skip {
            // Re-assert native styles and icons before the worker refreshes the tab.
            ensure_hwnd_shell_integration(hwnd);
            attach_hwnd_webview_keyboard_focus(hwnd);
        }
        crate::win_taskbar::set_tab(hwnd, !skip, Some(crate::win_taskbar_overlay::last_count()));
    });
}

fn ensure_hwnd_shell_integration(hwnd: HWND) {
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Release);
    apply_exe_window_icons(hwnd);
    unsafe {
        // Clear accidental owner (GWLP_HWNDPARENT on a top-level window is the owner).
        // Owned windows are often skipped by Show Desktop when alone.
        let owner_ptr = GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT);
        if owner_ptr != 0 {
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            tracing::debug!("win_shell: cleared window owner");
        }
        if let Ok(gw_owner) = GetWindow(hwnd, GW_OWNER) {
            if !gw_owner.0.is_null() {
                let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            }
        }

        let mut style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let mut ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut changed = false;

        if style & WS_MINIMIZEBOX.0 == 0 {
            style |= WS_MINIMIZEBOX.0;
            changed = true;
        }
        // Frameless HWNDs still need MAXIMIZEBOX so IsZoomed / SW_MAXIMIZE work.
        if style & WS_MAXIMIZEBOX.0 == 0 {
            style |= WS_MAXIMIZEBOX.0;
            changed = true;
        }
        // Visible app windows must not be tool windows — TOOLWINDOW alone is excluded
        // from Show Desktop's "significant window" set when it is the only one open.
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            ex &= !WS_EX_TOOLWINDOW.0;
            changed = true;
        }
        if ex & WS_EX_APPWINDOW.0 == 0 {
            ex |= WS_EX_APPWINDOW.0;
            changed = true;
        }
        let was_topmost = ex & WS_EX_TOPMOST.0 != 0;
        if was_topmost {
            ex &= !WS_EX_TOPMOST.0;
            changed = true;
        }

        if changed {
            SetWindowLongW(hwnd, GWL_STYLE, style as i32);
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex as i32);
        }

        // Always poke FRAMECHANGED so Explorer re-reads styles; drop TOPMOST z-order if needed.
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED;
        if was_topmost {
            let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
        } else {
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, flags | SWP_NOZORDER);
        }
    }
}

/// wry child-webview class (Tauri `unstable` / `build_as_child`).
const WRY_WEBVIEW_CLASS: &str = "WRY_WEBVIEW";
/// Stored original WndProc pointer (`SetWindowLongPtr` subclass).
const ORIG_PROC_PROP: PCWSTR = windows::core::w!("GrokWvKbdFocusOrig");
static FORWARDING_KEYBOARD_FOCUS: AtomicBool = AtomicBool::new(false);

/// Forward Alt-Tab / taskbar activation into the child WebView2 HWND.
///
/// Safe to call repeatedly (skips if the original WndProc prop is already set).
pub fn attach_webview_keyboard_focus(window: &WebviewWindow) {
    on_window_thread(window, |_, hwnd| attach_hwnd_webview_keyboard_focus(hwnd));
}

fn attach_hwnd_webview_keyboard_focus(hwnd: HWND) {
    unsafe {
        if !GetPropW(hwnd, ORIG_PROC_PROP).0.is_null() {
            return;
        }
        let prev = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            keyboard_focus_wndproc as *const () as isize,
        );
        if prev == 0 {
            return;
        }
        if SetPropW(
            hwnd,
            ORIG_PROC_PROP,
            Some(HANDLE(prev as *mut std::ffi::c_void)),
        )
        .is_err()
        {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, prev);
        }
    }
}

unsafe extern "system" fn keyboard_focus_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if should_handle_focus_message(msg, wparam.0 as u32) {
        forward_keyboard_focus_to_webview(hwnd);
    }
    let orig = GetPropW(hwnd, ORIG_PROC_PROP);
    if msg == WM_NCDESTROY {
        let _ = RemovePropW(hwnd, ORIG_PROC_PROP);
    }
    type WndProcFn = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
    let prev: WNDPROC = if orig.0.is_null() {
        None
    } else {
        Some(std::mem::transmute::<*mut std::ffi::c_void, WndProcFn>(
            orig.0,
        ))
    };
    CallWindowProcW(prev, hwnd, msg, wparam, lparam)
}

fn forward_keyboard_focus_to_webview(hwnd: HWND) {
    if FORWARDING_KEYBOARD_FOCUS.swap(true, Ordering::SeqCst) {
        return;
    }
    let _guard = ForwardingGuard;
    unsafe {
        let focus = GetFocus();
        if !focus.0.is_null() && IsChild(hwnd, focus).as_bool() {
            return;
        }
        if let Some(child) = first_visible_wry_webview_child(hwnd) {
            let _ = SetFocus(Some(child));
        }
    }
}

struct ForwardingGuard;
impl Drop for ForwardingGuard {
    fn drop(&mut self) {
        FORWARDING_KEYBOARD_FOCUS.store(false, Ordering::SeqCst);
    }
}

fn first_visible_wry_webview_child(parent: HWND) -> Option<HWND> {
    unsafe {
        let mut child = hwnd_or_none(GetWindow(parent, GW_CHILD).ok())?;
        loop {
            if IsWindowVisible(child).as_bool() && hwnd_is_wry_webview(child) {
                return Some(child);
            }
            child = hwnd_or_none(GetWindow(child, GW_HWNDNEXT).ok())?;
        }
    }
}

fn hwnd_or_none(hwnd: Option<HWND>) -> Option<HWND> {
    hwnd.filter(|h| !h.0.is_null())
}

fn hwnd_is_wry_webview(hwnd: HWND) -> bool {
    is_wry_webview_class(&hwnd_class_name(hwnd))
}

fn hwnd_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// `WM_SETFOCUS`, or `WM_ACTIVATE` that is not minimize / deactivate.
fn should_handle_focus_message(msg: u32, wparam: u32) -> bool {
    if msg == WM_SETFOCUS {
        return true;
    }
    if msg != WM_ACTIVATE {
        return false;
    }
    let state = wparam & 0xffff;
    let minimized = ((wparam >> 16) & 0xffff) != 0;
    !minimized && (state == WA_ACTIVE || state == WA_CLICKACTIVE)
}

fn is_wry_webview_class(name: &str) -> bool {
    name.eq_ignore_ascii_case(WRY_WEBVIEW_CLASS)
}

/// Pure helper for unit tests: Alt-Tab / Show-Desktop significance rules (simplified).
#[cfg(test)]
pub fn is_shell_significant_for_tests(style: u32, ex: u32, has_owner: bool) -> bool {
    let tool = ex & WS_EX_TOOLWINDOW.0 != 0;
    let app = ex & WS_EX_APPWINDOW.0 != 0;
    let minbox = style & WS_MINIMIZEBOX.0 != 0;
    if has_owner && !app {
        return false;
    }
    if tool && !app {
        return false;
    }
    minbox && (app || !tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATEAPP;

    #[test]
    fn overlay_skip_taskbar_clears_appwindow() {
        let ex = overlay_skip_taskbar_exstyle(WS_EX_APPWINDOW.0);
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, WS_EX_TOOLWINDOW.0);
        assert_eq!(ex & WS_EX_APPWINDOW.0, 0);
        let already = overlay_skip_taskbar_exstyle(WS_EX_TOOLWINDOW.0 | WS_EX_APPWINDOW.0);
        assert_eq!(already & WS_EX_APPWINDOW.0, 0);
        assert_eq!(already & WS_EX_TOOLWINDOW.0, WS_EX_TOOLWINDOW.0);
    }

    #[test]
    fn toolwindow_without_appwindow_is_not_significant() {
        let style = WS_MINIMIZEBOX.0;
        let ex = WS_EX_TOOLWINDOW.0;
        assert!(!is_shell_significant_for_tests(style, ex, false));
    }

    #[test]
    fn appwindow_with_minimize_is_significant() {
        let style = WS_MINIMIZEBOX.0;
        let ex = WS_EX_APPWINDOW.0;
        assert!(is_shell_significant_for_tests(style, ex, false));
    }

    #[test]
    fn owned_without_appwindow_is_not_significant() {
        let style = WS_MINIMIZEBOX.0;
        let ex = 0;
        assert!(!is_shell_significant_for_tests(style, ex, true));
    }

    #[test]
    fn owned_with_appwindow_is_significant() {
        let style = WS_MINIMIZEBOX.0;
        let ex = WS_EX_APPWINDOW.0;
        assert!(is_shell_significant_for_tests(style, ex, true));
    }

    #[test]
    fn wry_webview_class_matches_child_container() {
        assert!(is_wry_webview_class("WRY_WEBVIEW"));
        assert!(is_wry_webview_class("wry_webview"));
        assert!(!is_wry_webview_class("Chrome_WidgetWin_1"));
        assert!(!is_wry_webview_class(""));
    }

    #[test]
    fn alt_tab_activate_and_setfocus_forward_to_webview() {
        assert!(should_handle_focus_message(WM_SETFOCUS, 0));
        assert!(should_handle_focus_message(WM_ACTIVATE, WA_ACTIVE));
        assert!(should_handle_focus_message(WM_ACTIVATE, WA_CLICKACTIVE));
        assert!(!should_handle_focus_message(WM_ACTIVATE, 0));
        // HIWORD set → window is minimized while activating.
        assert!(!should_handle_focus_message(
            WM_ACTIVATE,
            WA_ACTIVE | (1 << 16)
        ));
        assert!(!should_handle_focus_message(WM_ACTIVATEAPP, WA_ACTIVE));
    }

    #[test]
    fn applies_native_integration_before_single_taskbar_enqueue() {
        let src = include_str!("win_shell.rs");
        let start = src.find("pub fn set_main_window_skip_taskbar(").unwrap();
        let end = src[start..]
            .find("\nfn ensure_hwnd_shell_integration(")
            .unwrap()
            + start;
        let restore = &src[start..end];
        assert_eq!(restore.matches("crate::win_taskbar::set_tab(").count(), 1);
        assert!(
            restore.find("ensure_hwnd_shell_integration(hwnd)").unwrap()
                < restore.find("crate::win_taskbar::set_tab(").unwrap()
        );
        let native_end = src[end..].find("/// wry child-webview class").unwrap() + end;
        let native = &src[end..native_end];
        assert!(native.contains("apply_exe_window_icons(hwnd)"));
        assert!(!native.contains("set_tab("));
    }
}
