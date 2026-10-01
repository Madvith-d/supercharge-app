/// Latency-critical custom-caption actions.
///
/// On Windows this bypasses Tauri's second window-message queue and posts the
/// state transition directly to the cached HWND. Other hosts retain Tauri's
/// normal API (Linux still needs its frontend work-area fallback for maximize).
#[tauri::command]
pub fn window_caption_action(
    window: tauri::WebviewWindow,
    action: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        if window.label() != "main" {
            return Err("caption action is only available for the main window".into());
        }
        return crate::win_shell::post_main_caption_action(&action);
    }

    #[cfg(not(windows))]
    {
        match action.as_str() {
            "minimize" => window.minimize().map_err(|e| e.to_string()),
            "toggleMaximize" => {
                if window.is_maximized().unwrap_or(false) {
                    window.unmaximize().map_err(|e| e.to_string())
                } else {
                    window.maximize().map_err(|e| e.to_string())
                }
            }
            _ => Err(format!("unsupported caption action: {action}")),
        }
    }
}
