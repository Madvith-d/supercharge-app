//! Verify that visible CLI turns use the same indexes as native rewind.

use std::path::Path;

use serde_json::Value;

use crate::store::SessionMeta;

const MAX_HISTORY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SUMMARY_BYTES: u64 = 256 * 1024;
const MAPPING_ERROR: &str = "Cannot safely map CLI history turns to native rewind indexes";

/// Pending children have a cut App journal but still point to the full source.
pub fn verify(meta: &SessionMeta) -> Result<u32, String> {
    let directory = super::history_directory(meta)
        .ok_or_else(|| format!("{MAPPING_ERROR}: native history is missing or ambiguous"))?;
    let count = verify_directory(&directory)?;
    if super::history_directory(meta).as_deref() != Some(directory.as_path()) {
        return Err(format!("{MAPPING_ERROR}: native history changed; retry"));
    }
    Ok(count)
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    super::read_regular(path, limit)
        .map_err(|_| format!("{MAPPING_ERROR}: missing, unsafe, or oversized native history"))
}

fn read_updates(directory: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = directory.join("updates.jsonl");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => read(&path, MAX_HISTORY_BYTES).map(Some),
    }
}

fn verify_directory(directory: &Path) -> Result<u32, String> {
    let summary_path = directory.join("summary.json");
    let history_path = directory.join("chat_history.jsonl");
    let summary = read(&summary_path, MAX_SUMMARY_BYTES)?;
    let value: Value = serde_json::from_slice(&summary)
        .map_err(|_| format!("{MAPPING_ERROR}: invalid native summary"))?;
    if value.get("chat_format_version").and_then(Value::as_u64) != Some(1) {
        return Err(format!(
            "{MAPPING_ERROR}: explicit v1 native prompt indexes required"
        ));
    }
    let history = read(&history_path, MAX_HISTORY_BYTES)?;
    let native_count = native_prompt_count(&history)?;
    let updates = read_updates(directory)?;
    let transcript = crate::cli_history_transcript::read_transcript(directory)?;
    let visible_count = crate::store::user_prompt_count(&transcript);
    if native_count != visible_count {
        return Err(format!(
            "{MAPPING_ERROR}: native prompts ({native_count}) differ from visible prompts ({visible_count}); hidden prompts or unindexed rows cannot be mapped"
        ));
    }
    if read(&summary_path, MAX_SUMMARY_BYTES)? != summary
        || read(&history_path, MAX_HISTORY_BYTES)? != history
        || read_updates(directory)? != updates
    {
        return Err(format!("{MAPPING_ERROR}: native history changed; retry"));
    }
    Ok(native_count)
}

fn native_prompt_count(history: &[u8]) -> Result<u32, String> {
    let mut count = 0u32;
    let mut prefix_seen = false;
    for line in history.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let item: Value = serde_json::from_slice(line)
            .map_err(|_| format!("{MAPPING_ERROR}: incomplete or invalid native history"))?;
        let role = item
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{MAPPING_ERROR}: missing v1 native message type"))?;
        if role != "user" {
            continue;
        }
        if let Some(index) = item.get("prompt_index").filter(|v| !v.is_null()) {
            if index.as_u64() != Some(u64::from(count)) {
                return Err(format!(
                    "{MAPPING_ERROR}: native prompt indexes are not contiguous from zero"
                ));
            }
            // Synthetic wake/scheduler turns consume indexes just like human turns.
            count = count
                .checked_add(1)
                .ok_or_else(|| format!("{MAPPING_ERROR}: native prompt count overflow"))?;
        } else if count == 0 {
            // CLI skips one unmarked Human preamble before its first indexed turn.
            match item
                .get("synthetic_reason")
                .and_then(Value::as_str)
                .unwrap_or("human")
            {
                "human" if !prefix_seen => prefix_seen = true,
                "primary"
                | "session_prefix"
                | "compaction_meta"
                | "system_reminder"
                | "length_continue"
                | "project_instructions"
                | "auto_continue"
                | "auto_recovery"
                | "interjection"
                | "goal_summary"
                | "stop_hook_feedback"
                | "working_directory_switch" => {}
                _ => return Err(format!("{MAPPING_ERROR}: missing native prompt index")),
            }
        }
        // After the first marker, unmarked rows are in-turn injections in CLI rewind.
    }
    if count == 0 {
        return Err(format!(
            "{MAPPING_ERROR}: no explicit native prompt indexes"
        ));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("cli-prompt-mapping-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let fixture = Self(directory);
            fixture.write(
                "summary.json",
                &json!({"chat_format_version": 1}).to_string(),
            );
            fixture
        }

        fn write(&self, name: &str, value: &str) {
            std::fs::write(self.0.join(name), value).unwrap();
        }

        fn history(&self, users: &[Value]) {
            let mut rows = vec![
                json!({"type":"system","content":"fixture"}),
                json!({"type":"user","content":[{"type":"text","text":"<user_info>fixture</user_info>"}]}),
            ];
            for user in users {
                rows.push(user.clone());
                rows.push(json!({"type":"assistant","content":"answer"}));
            }
            self.write(
                "chat_history.jsonl",
                &rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
            );
        }

        fn updates(&self, hidden: Option<usize>) {
            let mut rows = Vec::new();
            for index in 0..2 {
                rows.push(json!({"method":"session/update","params":{"sessionId":"fixture","update":{
                    "sessionUpdate":"user_message_chunk", "content":{"type":"text","text":format!("q{index}")},
                    "_meta":{"promptIndex":index,"hideFromScrollback":hidden == Some(index)}
                }}}));
                rows.push(json!({"method":"session/update","params":{"sessionId":"fixture","update":{
                    "sessionUpdate":"agent_message_chunk", "content":{"type":"text","text":"answer"}
                }}}));
            }
            self.write(
                "updates.jsonl",
                &rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn user(index: u32) -> Value {
        json!({"type":"user","prompt_index":index,"content":[{"type":"text","text":format!("q{index}")}]})
    }

    #[test]
    fn normal_v1_source_has_identity_mapping_even_with_a_cut_child_journal() {
        let fixture = Fixture::new();
        fixture.history(&[user(0), user(1)]);
        fixture.updates(None);
        fixture.write("messages.json", "[]");
        assert_eq!(verify_directory(&fixture.0), Ok(2));
    }

    #[test]
    fn normal_v1_without_updates_has_identity_mapping() {
        let fixture = Fixture::new();
        fixture.history(&[user(0), user(1)]);
        assert_eq!(verify_directory(&fixture.0), Ok(2));
    }

    #[test]
    fn hidden_indexed_prompt_is_not_silently_removed_from_native_count() {
        let fixture = Fixture::new();
        fixture.history(&[user(0), user(1)]);
        fixture.updates(Some(0));
        assert!(verify_directory(&fixture.0)
            .unwrap_err()
            .contains("hidden prompts"));
    }

    #[test]
    fn indexed_synthetic_wake_still_consumes_a_native_prompt() {
        let fixture = Fixture::new();
        let mut wake = user(1);
        wake["synthetic_reason"] = json!("task_completed");
        fixture.history(&[user(0), wake]);
        fixture.updates(None);
        assert_eq!(verify_directory(&fixture.0), Ok(2));
        fixture.updates(Some(1));
        assert!(verify_directory(&fixture.0)
            .unwrap_err()
            .contains("hidden prompts"));
    }

    #[test]
    fn filtered_indexed_instruction_is_unmappable() {
        let fixture = Fixture::new();
        let mut instruction = user(1);
        instruction["synthetic_reason"] = json!("project_instructions");
        fixture.history(&[user(0), instruction]);
        assert!(verify_directory(&fixture.0)
            .unwrap_err()
            .contains("hidden prompts"));
    }

    #[test]
    fn unindexed_mid_turn_instruction_does_not_consume_an_index() {
        let fixture = Fixture::new();
        fixture.history(&[
            user(0),
            json!({"type":"user", "synthetic_reason":"system_reminder",
            "content":[{"type":"text","text":"<system-reminder>fixture</system-reminder>"}]}),
            user(1),
        ]);
        fixture.updates(None);
        assert_eq!(verify_directory(&fixture.0), Ok(2));
    }

    #[test]
    fn unindexed_wake_before_first_marker_is_ambiguous() {
        let fixture = Fixture::new();
        fixture.history(&[
            json!({"type":"user", "synthetic_reason":"task_completed",
            "content":[{"type":"text","text":"wake"}]}),
            user(0),
        ]);
        assert!(verify_directory(&fixture.0)
            .unwrap_err()
            .contains("missing native prompt index"));
    }

    #[test]
    fn missing_duplicate_gapped_and_v0_indexes_are_rejected() {
        for users in [
            vec![user(1)],
            vec![user(0), user(0)],
            vec![user(0), user(2)],
            vec![json!({"type":"user","content":"unindexed"})],
        ] {
            let fixture = Fixture::new();
            fixture.history(&users);
            assert!(verify_directory(&fixture.0).is_err());
        }
        let fixture = Fixture::new();
        fixture.history(&[user(0)]);
        fixture.write("summary.json", "{\"chat_format_version\":0}");
        assert!(verify_directory(&fixture.0).unwrap_err().contains("v1"));
    }

    #[test]
    fn incomplete_native_record_and_oversized_history_are_rejected() {
        let fixture = Fixture::new();
        fixture.write("chat_history.jsonl", "{\"type\":\"user\"");
        assert!(verify_directory(&fixture.0).is_err());
        std::fs::File::create(fixture.0.join("chat_history.jsonl"))
            .unwrap()
            .set_len(MAX_HISTORY_BYTES + 1)
            .unwrap();
        assert!(verify_directory(&fixture.0)
            .unwrap_err()
            .contains("oversized"));
    }
}
