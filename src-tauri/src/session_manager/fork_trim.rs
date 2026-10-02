//! Policy for trimming a forked child agent so its memory matches a cut journal.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildTrimPlan {
    /// Full fork or journal-only: nothing to rewind.
    Skip,
    /// Exclusive native boundary; the stored/UI cut remains inclusive.
    RewindChild { prompt_index: u32 },
    /// Fork fell through to session/new; journal already truncated → bootstrap.
    Bootstrap,
}

pub fn child_trim_plan(rewind_index: Option<u32>, open_resumed: bool) -> ChildTrimPlan {
    match rewind_index {
        Some(through_index) if open_resumed => {
            match crate::store::inclusive_user_prompt_exec_index(through_index) {
                Ok(prompt_index) => ChildTrimPlan::RewindChild { prompt_index },
                Err(_) => ChildTrimPlan::Bootstrap,
            }
        }
        Some(_) => ChildTrimPlan::Bootstrap,
        None => ChildTrimPlan::Skip,
    }
}

/// Validate a pending native copy's UI cut before loading or rewinding it.
pub fn pending_child_rewind_exec_index(
    meta: &crate::store::SessionMeta,
) -> Result<Option<u32>, String> {
    meta.fork_rewind_prompt_index
        .map(|through_index| {
            crate::store::ensure_rewind_prompt_mapping(meta)?;
            crate::store::inclusive_user_prompt_exec_index(through_index)
        })
        .transpose()
}

/// Result of the rewind-fail journal fail-safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildRewindFailSafe {
    pub before_len: usize,
    pub after_len: usize,
    pub persisted: bool,
    pub need_bootstrap: bool,
}

/// After a successful re-cut, bootstrap the truncated journal into session/new.
/// Callers skip bootstrap only when the helper itself fails.
pub fn need_bootstrap_after_rewind_fail(after_len: usize) -> bool {
    after_len > 0
}

/// Re-cut after a failed exclusive native boundary, then decide bootstrap.
pub fn apply_child_rewind_fail_safe(
    session_id: &str,
    exclusive_prompt_index: u32,
) -> Result<ChildRewindFailSafe, String> {
    let through_user_prompt_index = exclusive_prompt_index
        .checked_sub(1)
        .ok_or("partial fork rewind boundary must retain at least one turn")?;
    let (before_len, after_len, persisted) =
        crate::store::retruncate_child_journal_to_cut(session_id, through_user_prompt_index)?;
    Ok(ChildRewindFailSafe {
        before_len,
        after_len,
        persisted,
        need_bootstrap: need_bootstrap_after_rewind_fail(after_len),
    })
}

/// After a failed child rewind, do not keep the untrimmed forked agent id.
#[cfg(test)]
pub fn child_trim_after_rewind_error(rewind_ok: bool) -> ChildTrimPlan {
    if rewind_ok {
        ChildTrimPlan::Skip
    } else {
        ChildTrimPlan::Bootstrap
    }
}

/// Host `session://fork_trimmed` outcome. `None` means do not emit (uncut fork).
pub fn fork_trimmed_outcome(plan: ChildTrimPlan, rewind_ok: Option<bool>) -> Option<&'static str> {
    match plan {
        ChildTrimPlan::Skip => None,
        ChildTrimPlan::Bootstrap => Some("bootstrap"),
        ChildTrimPlan::RewindChild { .. } => match rewind_ok {
            Some(true) => Some("rewound"),
            _ => Some("bootstrap"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        create_session, fork_session, load_messages, replace_messages, save_messages,
        update_session_meta, ChatMessageStored,
    };
    use chrono::Utc;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn trim_plan_rewinds_only_resumed_partial() {
        assert_eq!(
            child_trim_plan(Some(2), true),
            ChildTrimPlan::RewindChild { prompt_index: 3 }
        );
        assert_eq!(child_trim_plan(Some(2), false), ChildTrimPlan::Bootstrap);
        assert_eq!(child_trim_plan(None, true), ChildTrimPlan::Skip);
        assert_eq!(child_trim_plan(None, false), ChildTrimPlan::Skip);
        assert_eq!(
            child_trim_plan(Some(0), true),
            ChildTrimPlan::RewindChild { prompt_index: 1 }
        );
        assert_eq!(
            child_trim_plan(Some(u32::MAX), true),
            ChildTrimPlan::Bootstrap
        );
    }

    #[test]
    fn pending_native_cut_is_exclusive_and_rejects_unmapped_cli_history() {
        let mut meta: crate::store::SessionMeta = serde_json::from_value(serde_json::json!({
            "id": "child", "title": "child", "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z", "forkRewindPromptIndex": 0
        }))
        .unwrap();
        assert_eq!(pending_child_rewind_exec_index(&meta), Ok(Some(1)));
        meta.fork_rewind_prompt_index = Some(u32::MAX);
        assert!(pending_child_rewind_exec_index(&meta).is_err());
        meta.cli_source = Some(crate::cli_history::CliSessionSource {
            source_home: "/fixture".into(),
            relative_dir: "sessions/workspace/agent".into(),
            agent_session_id: "agent".into(),
            cwd: None,
            title: None,
            updated_at: None,
            revision: String::new(),
            app_owned: true,
        });
        meta.fork_rewind_prompt_index = Some(0);
        assert!(pending_child_rewind_exec_index(&meta)
            .unwrap_err()
            .contains("Cannot safely map"));
        meta.fork_rewind_prompt_index = None;
        assert_eq!(pending_child_rewind_exec_index(&meta), Ok(None));
    }

    #[test]
    fn fail_safe_rejects_zero_boundary_before_touching_the_journal() {
        assert!(apply_child_rewind_fail_safe("missing-child", 0)
            .unwrap_err()
            .contains("retain at least one turn"));
    }

    #[test]
    fn rewind_error_forces_bootstrap() {
        assert_eq!(child_trim_after_rewind_error(true), ChildTrimPlan::Skip);
        assert_eq!(
            child_trim_after_rewind_error(false),
            ChildTrimPlan::Bootstrap
        );
    }

    #[test]
    fn cut_journal_bootstraps_after_rewind_fail() {
        assert!(need_bootstrap_after_rewind_fail(10));
        assert!(!need_bootstrap_after_rewind_fail(0));
    }

    #[test]
    fn trimmed_outcome_is_silent_on_uncut_and_honest_on_bootstrap() {
        assert_eq!(fork_trimmed_outcome(ChildTrimPlan::Skip, None), None);
        assert_eq!(
            fork_trimmed_outcome(ChildTrimPlan::Bootstrap, None),
            Some("bootstrap")
        );
        assert_eq!(
            fork_trimmed_outcome(ChildTrimPlan::RewindChild { prompt_index: 0 }, Some(true)),
            Some("rewound")
        );
        assert_eq!(
            fork_trimmed_outcome(ChildTrimPlan::RewindChild { prompt_index: 0 }, Some(false)),
            Some("bootstrap")
        );
        assert_eq!(
            fork_trimmed_outcome(ChildTrimPlan::RewindChild { prompt_index: 0 }, None),
            Some("bootstrap")
        );
    }

    fn stored_msg(id: &str, role: &str, content: &str) -> ChatMessageStored {
        ChatMessageStored {
            id: id.into(),
            role: role.into(),
            content: content.into(),
            thought: None,
            created_at: Utc::now(),
            is_error: false,
            attachments: None,
            marker: None,
        }
    }

    #[test]
    fn rewind_fail_inflated_child_bootstraps_cut_and_keeps_ten_turns() {
        let _g = crate::paths::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "grok-app-rewind-fail-safe-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("tmp home");
        std::env::set_var("GROK_APP_HOME", &tmp);
        let _ = crate::paths::ensure_app_dirs();

        let mut src = create_session(None, Some("src".into()), false).expect("create");
        src.agent_session_id = Some("agent-parent".into());
        update_session_meta(&src).expect("meta");
        let parent_msgs: Vec<_> = (1u32..=8)
            .flat_map(|i| {
                [
                    stored_msg(&format!("u{i}"), "user", &format!("q{i}")),
                    stored_msg(&format!("a{i}"), "assistant", &format!("a{i}")),
                ]
            })
            .collect();
        save_messages(&src.id, &parent_msgs).expect("msgs");
        let child = fork_session(&src.id, Some(4), None, true).expect("partial");
        replace_messages(&child.id, &parent_msgs).expect("inflate");
        assert_eq!(load_messages(&child.id).len(), 16);

        let ChildTrimPlan::RewindChild { prompt_index } =
            child_trim_plan(child.fork_rewind_prompt_index, true)
        else {
            panic!("partial fork must rewind");
        };
        assert_eq!(prompt_index, 5);
        let fs = apply_child_rewind_fail_safe(&child.id, prompt_index).expect("fail-safe");
        assert_eq!(fs.before_len, 16);
        assert_eq!(fs.after_len, 10);
        assert!(fs.persisted);
        assert!(
            fs.need_bootstrap,
            "cut journal must bootstrap after rewind fail"
        );
        let kept = load_messages(&child.id);
        assert_eq!(kept.len(), 10);
        assert_eq!(kept.last().map(|m| m.content.as_str()), Some("a5"));

        std::env::remove_var("GROK_APP_HOME");
        let _ = fs::remove_dir_all(&tmp);
    }
}
