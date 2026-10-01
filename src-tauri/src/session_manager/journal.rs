//! Rewind timeline and journal checkpoints.

use std::sync::Arc;

use tauri::AppHandle;

use crate::acp_client::AcpClient;
use crate::error::AgentErrorCode;
use crate::session_fsm::SessionState;
use crate::store::{self};

use super::*;

/// Budget for a single agent rewind RPC. `AcpClient::request` already allows
/// `HANDSHAKE_TIMEOUT_SECS` per method name and `rewind_execute_for` probes
/// several names, so an unbounded await can park a rewind command for minutes
/// with the rollback dialog spinning. The local journal is the UI source of
/// truth, so exceeding the budget degrades to "agent rewind failed" instead.
const REWIND_AGENT_RPC_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

fn read_rewind_messages(app_sid: &str) -> Result<Vec<store::ChatMessageStored>, String> {
    Ok(
        crate::cli_history::read_messages(app_sid)?
            .unwrap_or_else(|| store::load_messages(app_sid)),
    )
}

fn materialize_rewind_journal(app_sid: &str) -> Result<Vec<store::ChatMessageStored>, String> {
    let meta = store::load_sessions_index()
        .into_iter()
        .find(|meta| meta.id == app_sid);
    if let Some(meta) = meta.as_ref() {
        store::ensure_rewind_prompt_mapping(meta)?;
        if meta.cli_source.as_ref().is_some_and(|source| {
            !source.app_owned || meta.agent_session_id.as_deref().is_none_or(str::is_empty)
        }) {
            return Err("Continue this CLI session before rewinding its native history".into());
        }
        crate::cli_history::materialize_for_app(app_sid)
    } else {
        read_rewind_messages(app_sid)
    }
}

impl SessionManager {
    async fn prepare_cli_rewind(
        self: &Arc<Self>,
        app: &AppHandle,
        app_sid: &str,
    ) -> Result<(), String> {
        Self::ensure_rewind_mapping(app_sid)?;
        let needs_copy = store::load_sessions_index()
            .into_iter()
            .find(|meta| meta.id == app_sid)
            .is_some_and(|meta| {
                meta.cli_source.as_ref().is_some_and(|source| {
                    !source.app_owned || meta.agent_session_id.as_deref().is_none_or(str::is_empty)
                })
            });
        if needs_copy {
            self.connect(app.clone(), None, Some(app_sid.to_owned()), None, None)
                .await?;
        }
        Self::ensure_rewind_mapping(app_sid)
    }

    fn ensure_rewind_mapping(app_sid: &str) -> Result<(), String> {
        if let Some(meta) = store::load_sessions_index()
            .into_iter()
            .find(|meta| meta.id == app_sid)
        {
            store::ensure_rewind_prompt_mapping(&meta)?;
        }
        Ok(())
    }

    /// True when a rewind must be refused because a turn is still running for
    /// `app_sid` — whether that chat holds the live slot or was demoted to
    /// background when the user switched away.
    ///
    /// Reading only `inner` reported idle for a demoted chat, so the journal was
    /// truncated underneath a turn that was still streaming and the agent's
    /// memory stopped matching the transcript. `live_session_is_busy` is also
    /// authoritative where the FSM is not: it honours `prompt_in_flight` after
    /// an early `prompt_complete`.
    pub(super) fn rewind_blocked_by_running_turn(&self, app_sid: &str) -> bool {
        self.with_session_mut(app_sid, |s| Self::live_session_is_busy(s))
            .unwrap_or(false)
    }

    pub async fn rewind_drop_last_user_turn(
        self: &Arc<Self>,
        app: AppHandle,
        session_id: Option<String>,
    ) -> Result<SessionSnapshot, String> {
        let app_sid = session_id
            .clone()
            .filter(|id| !id.trim().is_empty())
            .or_else(|| self.inner.lock().as_ref().map(|s| s.app_session_id.clone()))
            .ok_or("no active session")?;
        if self.rewind_blocked_by_running_turn(&app_sid) {
            return Err("cannot edit while a turn is running".into());
        }
        self.prepare_cli_rewind(&app, &app_sid).await?;
        let (backend, app_sid, acp, agent_sid, user_prompt_count) = {
            let guard = self.inner.lock();
            let s = guard.as_ref().ok_or("no active session")?;
            if s.app_session_id != app_sid {
                return Err(format!(
                    "{}: chat {app_sid} is not focused — reconnect and retry",
                    AgentErrorCode::ConnectFailed.as_str()
                ));
            }
            if s.fsm.state() == SessionState::Streaming
                || s.fsm.state() == SessionState::AwaitingPermission
            {
                return Err("cannot edit while a turn is running".into());
            }
            let msgs = read_rewind_messages(&s.app_session_id)?;
            let user_prompt_count = store::user_prompt_count(&msgs);
            if user_prompt_count == 0 {
                return Err("no user message to rewind".into());
            }
            (
                s.backend.clone(),
                s.app_session_id.clone(),
                s.acp.clone(),
                s.meta.agent_session_id.clone(),
                user_prompt_count,
            )
        };

        Self::ensure_rewind_mapping(&app_sid)?;
        materialize_rewind_journal(&app_sid)?;

        // Native rewind discards the target prompt and everything after it.
        let mut agent_rewind_ok: Option<bool> = None;
        if backend != "mock_acp" && !AcpClient::use_mock() {
            let client = acp.ok_or("agent not connected; local journal left intact")?;
            let exec_index = store::drop_last_user_prompt_exec_index(user_prompt_count)
                .ok_or("no user message to rewind")?;
            let sid = agent_sid.as_deref().ok_or("chat has no agent session id")?;
            match tokio::time::timeout(
                REWIND_AGENT_RPC_BUDGET,
                client.rewind_execute_for(sid, exec_index, false),
            )
            .await
            {
                Ok(Ok(_)) => {
                    agent_rewind_ok = Some(true);
                    tracing::info!(
                        target: "session",
                        "rewind_drop_last_user_turn: agent rewound target={exec_index} (user_turns={user_prompt_count})"
                    );
                }
                Ok(Err(e)) => {
                    return Err(format!(
                        "agent rewind failed; local journal left intact: {e}"
                    ));
                }
                Err(_) => {
                    return Err("agent rewind timed out; local journal left intact".into());
                }
            }
        }

        if !store::drop_last_should_truncate_journal(agent_rewind_ok) {
            return Err("agent rewind failed; local journal left intact".into());
        }

        // Local journal: keep messages strictly before the last *prompt* user message.
        let msgs = store::load_messages(&app_sid);
        let cut = store::cut_index_before_last_user_prompt(&msgs);
        let kept: Vec<_> = msgs.into_iter().take(cut).collect();
        store::replace_messages(&app_sid, &kept)?;

        {
            let mut guard = self.inner.lock();
            if let Some(s) = guard.as_mut() {
                s.meta.updated_at = chrono::Utc::now();
                let _ = store::update_session_meta(&s.meta);
            }
        }
        let snap = self.snapshot();
        Self::emit_state(&app, &snap);
        Ok(snap)
    }

    /// List rewind points for an app session journal (one per user prompt).
    /// Prefer the local journal so the UI timeline always matches what the user sees.
    pub fn list_rewind_points(
        &self,
        session_id: Option<String>,
    ) -> Result<Vec<RewindPointDto>, String> {
        let app_sid = match session_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => {
                let guard = self.inner.lock();
                let s = guard.as_ref().ok_or("no active session")?;
                s.app_session_id.clone()
            }
        };
        // Ensure session exists in the index (or at least has a journal dir).
        let known = store::load_sessions_index().iter().any(|s| s.id == app_sid);
        let points = Self::rewind_points_from_journal(&app_sid)?;
        if !known && points.is_empty() {
            return Err(format!("session not found: {app_sid}"));
        }
        Ok(points)
    }

    pub(super) fn rewind_points_from_journal(app_sid: &str) -> Result<Vec<RewindPointDto>, String> {
        let msgs = read_rewind_messages(app_sid)?;
        let mut out = Vec::new();
        let mut idx = 0u32;
        for m in msgs {
            if !store::is_user_prompt_message(&m) {
                continue;
            }
            let raw = m.content.split_whitespace().collect::<Vec<_>>().join(" ");
            let preview = if raw.chars().count() > 80 {
                let truncated: String = raw.chars().take(79).collect();
                format!("{truncated}…")
            } else if raw.is_empty() {
                "…".into()
            } else {
                raw
            };
            out.push(RewindPointDto {
                prompt_index: idx,
                message_id: Some(m.id),
                preview,
            });
            idx = idx.saturating_add(1);
        }
        Ok(out)
    }

    /// Rewind a session to a user-prompt index (keep that turn, drop after).
    /// Live agent rewind must succeed before the local journal is truncated.
    pub async fn rewind_to_prompt_index(
        self: &Arc<Self>,
        app: AppHandle,
        target_prompt_index: u32,
        restore_files: bool,
        session_id: Option<String>,
    ) -> Result<RewindExecuteResult, String> {
        let app_sid = match session_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => {
                let guard = self.inner.lock();
                let s = guard.as_ref().ok_or("no active session")?;
                s.app_session_id.clone()
            }
        };

        if self.rewind_blocked_by_running_turn(&app_sid) {
            return Err("cannot rewind while a turn is running".into());
        }

        let msgs = read_rewind_messages(&app_sid)?;
        let user_count = store::user_prompt_count(&msgs);
        if user_count == 0 {
            return Err("no user messages to rewind".into());
        }
        if target_prompt_index >= user_count {
            return Err(format!(
                "user prompt index out of range: {target_prompt_index} (have {user_count})"
            ));
        }

        let exec_index = store::through_user_prompt_exec_index(target_prompt_index, user_count)?;
        if exec_index.is_none() {
            return Ok(RewindExecuteResult {
                snapshot: self.snapshot(),
                agent_ok: true,
                agent_error: None,
                local_ok: true,
                kept_count: msgs.len(),
            });
        }
        self.prepare_cli_rewind(&app, &app_sid).await?;
        let msgs = materialize_rewind_journal(&app_sid)?;
        let kept = store::truncate_through_user_prompt(&msgs, target_prompt_index)?;
        let exec_index = exec_index.expect("latest turn returned above");
        let (live_match, backend, acp, agent_sid) = {
            let guard = self.inner.lock();
            match guard.as_ref() {
                Some(s) if s.app_session_id == app_sid => (
                    true,
                    s.backend.clone(),
                    s.acp.clone(),
                    s.meta.agent_session_id.clone(),
                ),
                _ => (false, String::new(), None, None),
            }
        };
        let mut agent_ok = true;
        let mut agent_error: Option<String> = None;

        // Never guess another boundary after a failed native rewind.
        if live_match && backend != "mock_acp" && !AcpClient::use_mock() {
            if let (Some(client), Some(sid)) = (acp, agent_sid) {
                match tokio::time::timeout(
                    REWIND_AGENT_RPC_BUDGET,
                    client.rewind_execute_for(&sid, exec_index, restore_files),
                )
                .await
                {
                    Ok(Ok(_)) => {
                        tracing::info!(
                            target: "session",
                            "rewind_to_prompt_index: agent rewound boundary={exec_index} through={target_prompt_index}"
                        );
                    }
                    Ok(Err(e)) => {
                        return Err(format!(
                            "agent rewind failed; local journal left intact: {e}"
                        ));
                    }
                    Err(_) => {
                        return Err("agent rewind timed out; local journal left intact".into());
                    }
                }
            } else {
                return Err("agent not connected; local journal left intact".into());
            }
        } else if !live_match {
            agent_ok = false;
            agent_error = Some("session not live; local journal only".into());
        }

        let kept_count = kept.len();
        store::replace_messages(&app_sid, &kept)?;

        // Touch meta updated_at for index sort.
        let updated_at = chrono::Utc::now();
        if let Ok(Some(meta)) = store::update_sessions_index(|list| {
            let Some(meta) = list.iter_mut().find(|s| s.id == app_sid) else {
                return Ok(None);
            };
            meta.updated_at = updated_at;
            Ok(Some(meta.clone()))
        }) {
            if live_match {
                let mut guard = self.inner.lock();
                if let Some(s) = guard.as_mut() {
                    if s.app_session_id == app_sid {
                        s.meta.updated_at = meta.updated_at;
                    }
                }
            }
        }

        let snap = self.snapshot();
        Self::emit_state(&app, &snap);
        Ok(RewindExecuteResult {
            snapshot: snap,
            agent_ok,
            agent_error,
            local_ok: true,
            kept_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewind_rejects_cli_projection_before_materializing_or_truncating() {
        let _guard = crate::paths::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("rewind-mapping-{}", uuid::Uuid::new_v4()));
        let previous_home = std::env::var_os("GROK_APP_HOME");
        std::env::set_var("GROK_APP_HOME", &home);
        crate::paths::ensure_app_dirs().unwrap();
        let mut meta = store::create_session(None, None, false).unwrap();
        meta.agent_session_id = Some("execution-copy".into());
        meta.cli_source = Some(crate::cli_history::CliSessionSource {
            source_home: "/fixture".into(),
            relative_dir: "sessions/workspace/origin".into(),
            agent_session_id: "origin".into(),
            cwd: None,
            title: None,
            updated_at: None,
            revision: String::new(),
            app_owned: true,
        });
        store::update_session_meta(&meta).unwrap();
        let path = crate::paths::session_dir(&meta.id).join("messages.json");
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists());
        assert!(SessionManager::ensure_rewind_mapping(&meta.id).is_err());
        assert!(materialize_rewind_journal(&meta.id)
            .unwrap_err()
            .contains("Cannot safely map"));
        assert!(!path.exists(), "unsafe mapping must not claim a journal");
        store::replace_messages(&meta.id, &[]).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(materialize_rewind_journal(&meta.id).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        if let Some(previous) = previous_home {
            std::env::set_var("GROK_APP_HOME", previous);
        } else {
            std::env::remove_var("GROK_APP_HOME");
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
