use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cli-transcript-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, raw: impl AsRef<[u8]>) {
        fs::write(self.0.join(name), raw).unwrap();
    }
    fn read(&self) -> Result<Vec<ChatMessageStored>, String> {
        read_transcript(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn event(update: Value) -> Value {
    json!({"timestamp":1700000000,"method":"session/update","params":{"sessionId":"fixture-session","update":update}})
}
fn extension(update: Value) -> Value {
    let mut value = event(update);
    value["method"] = json!("_x.ai/session/update");
    value
}
fn text(role: &str, text: &str, pi: Option<u64>) -> Value {
    let mut update = json!({"sessionUpdate":role,"content":{"type":"text","text":text}});
    if let Some(pi) = pi {
        update["_meta"] = json!({"promptIndex":pi});
    }
    event(update)
}
fn user(text_value: &str, pi: u64) -> Value {
    text("user_message_chunk", text_value, Some(pi))
}
fn assistant(text_value: &str) -> Value {
    text("agent_message_chunk", text_value, None)
}
fn rewind(target: u64) -> Value {
    extension(
        json!({"sessionUpdate":"rewind_marker","target_prompt_index":target,"created_at":"2026-01-01T00:00:00Z"}),
    )
}
fn jsonl(events: &[Value]) -> String {
    events.iter().map(|v| format!("{v}\n")).collect()
}
fn parse(events: &[Value]) -> Vec<ChatMessageStored> {
    updates::parse(
        Path::new("/fixture/session"),
        records(jsonl(events).as_bytes(), "updates.jsonl").unwrap(),
    )
    .unwrap()
}

#[test]
fn installed_schema_preserves_chunks_thought_tools_and_timestamps() {
    // Shapes from d7a1754e63c2's persistence envelope, turn echo, and ACP tool builders.
    // All payloads here are authored fixtures, not captured conversations.
    let raw = r#"{"timestamp":1700000000,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Read the fixture."},"_meta":{"modelId":"fixture-model","promptIndex":0}},"_meta":{"eventId":"fixture-session:1","agentTimestampMs":1700000000123}}}
{"timestamp":1700000001,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Inspecting."}},"_meta":{"promptId":"fixture-prompt","eventId":"fixture-session:2","agentTimestampMs":1700000001001}}}
{"timestamp":1700000001,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I will "}}}}
{"timestamp":1700000001,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"read it."}}}}
{"timestamp":1700000002,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"Read fixture","kind":"read","status":"in_progress","rawInput":{"target_file":"/fixture/readme.txt"},"_meta":{"x.ai/tool":{"name":"read_file","label":"Read","kind":"read"}}}}}
{"timestamp":1700000003,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"Fixture contents."}}],"rawOutput":{"ignored":"not a duplicate output"}}}}
{"timestamp":1700000004,"method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Finished."}}}}
{"timestamp":1700000005,"method":"_x.ai/session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"turn_completed","prompt_id":"fixture-prompt","stop_reason":"end_turn","agent_result":"Finished."}}}
"#;
    let rows = updates::parse(
        Path::new("/fixture/session"),
        records(raw.as_bytes(), "updates.jsonl").unwrap(),
    )
    .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].created_at.timestamp_millis(), 1700000000123);
    assert_eq!(rows[1].content, "I will read it.");
    assert_eq!(rows[1].thought.as_deref(), Some("Inspecting."));
    assert_eq!(rows[1].created_at.timestamp_millis(), 1700000001001);
    assert!(rows[2]
        .content
        .starts_with("tool_step|completed|read_file|Read\ninput:"));
    assert!(rows[2].content.ends_with("Fixture contents."));
    assert!(!rows[2].content.contains("duplicate"));
    assert_eq!(rows[3].content, "Finished.");
}

#[test]
fn identical_prompts_are_distinct_even_when_adjacent() {
    let rows = parse(&[
        user("again", 0),
        user("again", 1),
        assistant("same"),
        user("again", 2),
        assistant("same"),
    ]);
    assert_eq!(rows.len(), 5);
    let ids: std::collections::HashSet<_> = rows.iter().map(|r| &r.id).collect();
    assert_eq!(ids.len(), rows.len());
}

#[test]
fn legacy_unindexed_chunks_join_but_turn_boundaries_do_not() {
    let rows = parse(&[
        text("user_message_chunk", "hel", None),
        text("user_message_chunk", "lo", None),
        assistant("ok"),
        text("user_message_chunk", "hello", None),
    ]);
    assert_eq!(
        rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(),
        ["hello", "ok", "hello"]
    );
    assert_ne!(rows[0].id, rows[2].id);
}

#[test]
fn message_id_and_response_boundaries_keep_assistant_messages_distinct() {
    let mut a = assistant("same");
    a["params"]["update"]["messageId"] = json!("message-1");
    let mut b = a.clone();
    b["params"]["update"]["messageId"] = json!("message-2");
    let rows = parse(&[
        a,
        b.clone(),
        extension(json!({"sessionUpdate":"response_completed"})),
        b,
    ]);
    assert_eq!(rows.len(), 3);
}

#[test]
fn chunked_and_wrapped_instruction_envelopes_do_not_become_bubbles() {
    let mut synthetic = user("do not display", 1);
    synthetic["params"]["update"]["_meta"]["synthetic_reason"] = json!("project_instructions");
    let mut hidden = user("hidden", 2);
    hidden["params"]["update"]["_meta"]["hideFromScrollback"] = json!(true);
    let mut host = user("host event", 3);
    host["params"]["update"]["_meta"]["hostTurn"] = json!(true);
    let rows = parse(&[
        user("<system-rem", 0),
        user("inder>hidden</system-reminder>", 0),
        synthetic,
        hidden,
        host,
        user(
            "<system-reminder>rules</system-reminder><user_query>Actual question</user_query>",
            4,
        ),
        user("<fork-context>old history</fork-context>New question", 5),
        user("<resume-context>old history</resume-context>", 6),
        user("<system-reminder>unfinished", 7),
    ]);
    assert_eq!(
        rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(),
        ["Actual question", "New question"]
    );
}

#[test]
fn attachments_are_in_memory_and_do_not_grant_or_materialize_files() {
    let rows = parse(&[
        user("Images\n@/fixture/local image.png", 0),
        event(
            json!({"sessionUpdate":"user_message_chunk","_meta":{"promptIndex":0},"content":{"type":"resource_link","uri":"file:///fixture/local%20image.png","name":"local image.png"}}),
        ),
        event(
            json!({"sessionUpdate":"user_message_chunk","_meta":{"promptIndex":0},"content":{"type":"image","mimeType":"image/png","data":"aGVsbG8="}}),
        ),
        event(
            json!({"sessionUpdate":"user_message_chunk","_meta":{"promptIndex":0},"content":{"type":"resource","resource":{"uri":"https://example.test/fixture.txt","text":"not a second bubble"}}}),
        ),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "Images");
    let paths: Vec<_> = rows[0]
        .attachments
        .as_ref()
        .unwrap()
        .iter()
        .map(|a| a.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "/fixture/local image.png",
            "data:image/png;base64,aGVsbG8=",
            "https://example.test/fixture.txt"
        ]
    );
}

#[test]
fn attachment_only_user_and_windows_file_uri_survive() {
    let rows = parse(&[event(
        json!({"sessionUpdate":"user_message_chunk","content":{"type":"resource_link","uri":"file:///C:/fixture/a%20b.txt","name":"a b.txt"}}),
    )]);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].content.is_empty());
    assert_eq!(
        rows[0].attachments.as_ref().unwrap()[0].path,
        "C:/fixture/a b.txt"
    );
}

#[test]
fn editor_context_does_not_create_attachment_bubbles() {
    assert!(parse(&[event(json!({"sessionUpdate":"user_message_chunk","content":{"type":"resource_link","uri":"file:///fixture/a.txt","name":"a.txt","_meta":{"source":"editor"}}}))]).is_empty());
}

#[test]
fn unknown_content_and_unsafe_uris_fail_without_echoing_payloads() {
    for block in [
        json!({"type":"future","text":"SECRET"}),
        json!({"type":"resource_link","uri":"javascript:SECRET"}),
        json!({"type":"image","data":"SECRET","mimeType":"image/png"}),
    ] {
        let raw = jsonl(&[event(
            json!({"sessionUpdate":"user_message_chunk","content":block}),
        )]);
        let error = updates::parse(
            Path::new("/fixture"),
            records(raw.as_bytes(), "updates.jsonl").unwrap(),
        )
        .unwrap_err();
        assert!(!error.contains("SECRET"));
    }
}

#[test]
fn tool_collections_replace_and_sparse_updates_preserve_identity() {
    let rows = parse(&[
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Read","kind":"read","status":"pending"}),
        ),
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":"partial"}}]}),
        ),
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","content":[{"type":"content","content":{"type":"text","text":"replacement"}}]}),
        ),
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"failed","title":null}),
        ),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "tool_step|failed|read|Read\nreplacement");
    assert!(rows[0].is_error);
}

#[test]
fn orphan_and_unfinished_tools_do_not_invent_completion() {
    let rows = parse(&[
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"orphan","rawOutput":"result","status":"completed"}),
        ),
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"running","title":"Run","status":"in_progress"}),
        ),
    ]);
    assert!(rows[0].content.ends_with("\nresult"));
    assert!(rows[1].content.starts_with("tool_step|in_progress|"));
}

#[test]
fn tool_media_is_structured_not_extracted_from_arbitrary_stdout() {
    let rows = parse(&[
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Media","content":[{"type":"content","content":{"type":"image","uri":"https://example.test/image.png","mimeType":"image/png","data":""}}]}),
        ),
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"u","title":"Run","rawOutput":"https://example.test/not-an-attachment.png"}),
        ),
    ]);
    assert_eq!(rows[0].attachments.as_ref().unwrap().len(), 1);
    assert!(rows[1].attachments.is_none());
}

#[test]
fn rewind_discards_target_prompt_and_all_later_events_then_rebranches() {
    let rows = parse(&[
        user("first", 0),
        assistant("a"),
        user("dead", 1),
        assistant("b"),
        rewind(1),
        user("replacement", 1),
        assistant("c"),
        user("also dead", 2),
        rewind(1),
        user("final", 1),
    ]);
    assert_eq!(
        rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(),
        ["first", "a", "final"]
    );
}

#[test]
fn rewind_restores_prior_tool_state_not_just_row_count() {
    let rows = parse(&[
        user("first", 0),
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Run","status":"in_progress"}),
        ),
        user("dead", 1),
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed","rawOutput":"dead output"}),
        ),
        rewind(1),
    ]);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].content.starts_with("tool_step|in_progress|"));
    assert!(!rows[1].content.contains("dead output"));
}

#[test]
fn rewind_tracks_hidden_prompts_and_does_not_count_host_turns_or_phantoms() {
    let mut hidden = user("hidden prompt", 1);
    hidden["params"]["update"]["_meta"]["hideFromScrollback"] = json!(true);
    let mut host = user("host", 99);
    host["params"]["update"]["_meta"]["hostTurn"] = json!(true);
    let rows = parse(&[
        user("keep", 0),
        assistant("a"),
        host,
        text(
            "user_message_chunk",
            "<system-reminder>phantom</system-reminder>",
            None,
        ),
        assistant("kept too"),
        hidden,
        assistant("dead"),
        rewind(1),
    ]);
    assert_eq!(
        rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(),
        ["keep", "a", "kept too"]
    );
}

#[test]
fn rewind_zero_clears_history_and_out_of_range_is_noop() {
    assert!(parse(&[user("gone", 0), rewind(0)]).is_empty());
    assert_eq!(parse(&[user("kept", 0), rewind(99)]).len(), 1);
}

#[test]
fn reread_append_truncate_and_rewrite_are_deterministic() {
    let fixture = Fixture::new();
    let prefix = jsonl(&[user("first", 0), assistant("answer")]);
    fixture.write("updates.jsonl", &prefix);
    let before = fixture.read().unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(fixture.read().unwrap()).unwrap()
    );
    fixture.write(
        "updates.jsonl",
        format!("{prefix}{}", jsonl(&[user("next", 1)])),
    );
    let appended = fixture.read().unwrap();
    assert_eq!(before[0].id, appended[0].id);
    assert_eq!(before[1].id, appended[1].id);
    fixture.write("updates.jsonl", &prefix);
    assert_eq!(fixture.read().unwrap().len(), 2);
    fixture.write("updates.jsonl", jsonl(&[user("replacement", 0)]));
    assert_eq!(fixture.read().unwrap()[0].content, "replacement");
}

#[test]
fn partial_tail_is_tolerated_but_complete_corruption_is_not() {
    let fixture = Fixture::new();
    let prefix = jsonl(&[user("kept", 0)]);
    fixture.write("updates.jsonl", format!("{prefix}{{\"params\":{{"));
    assert_eq!(fixture.read().unwrap().len(), 1);
    fixture.write("updates.jsonl", format!("{prefix}{{\"params\":{{\n"));
    assert!(fixture.read().unwrap_err().contains("line 2"));
    fixture.write("updates.jsonl", format!("{prefix}{{broken}}"));
    assert!(fixture.read().is_err());
    fixture.write(
        "updates.jsonl",
        jsonl(&[user("unterminated valid", 0)]).trim_end(),
    );
    assert_eq!(fixture.read().unwrap().len(), 1);
}

#[test]
fn partial_utf8_tail_and_crlf_are_supported() {
    let fixture = Fixture::new();
    let prefix = jsonl(&[user("世界", 0)]).replace('\n', "\r\n");
    let mut raw = format!("{prefix}{{\"text\":\"").into_bytes();
    raw.extend_from_slice(&[0xe4, 0xb8]);
    fixture.write("updates.jsonl", raw);
    assert_eq!(fixture.read().unwrap()[0].content, "世界");
}

#[test]
fn updates_are_authoritative_even_empty_partial_or_unsupported() {
    let fixture = Fixture::new();
    fixture.write(
        "chat_history.jsonl",
        "{\"type\":\"user\",\"content\":\"must not fall back\"}\n",
    );
    for raw in ["", "{\"params\":"] {
        fixture.write("updates.jsonl", raw);
        assert!(fixture.read().unwrap().is_empty());
    }
    fixture.write(
        "updates.jsonl",
        jsonl(&[event(json!({"sessionUpdate":"future_event"}))]),
    );
    assert!(fixture.read().is_err());
    fixture.write("updates.jsonl", "not JSON\n");
    assert!(fixture.read().is_err());
}

#[test]
fn missing_nonregular_and_oversized_files_have_explicit_errors() {
    let fixture = Fixture::new();
    assert!(fixture.read().unwrap_err().contains("missing"));
    fs::create_dir(fixture.0.join("updates.jsonl")).unwrap();
    assert!(fixture.read().is_err());
    fs::remove_dir(fixture.0.join("updates.jsonl")).unwrap();
    let file = fs::File::create(fixture.0.join("updates.jsonl")).unwrap();
    file.set_len(MAX_TRANSCRIPT_BYTES + 1).unwrap();
    assert!(fixture.read().unwrap_err().contains("128 MiB"));
}

#[test]
fn raw_legacy_notification_and_missing_timestamps_are_supported() {
    let raw = json!({"sessionId":"fixture-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"legacy"}}});
    let rows = parse(&[raw]);
    assert_eq!(rows[0].created_at, DateTime::<Utc>::UNIX_EPOCH);
}

#[test]
fn rfc3339_timestamps_and_invalid_timestamps() {
    let mut value = user("a", 0);
    value["timestamp"] = json!("2026-01-01T01:00:00+01:00");
    assert_eq!(
        parse(&[value.clone()])[0].created_at.to_rfc3339(),
        "2026-01-01T00:00:00+00:00"
    );
    value["timestamp"] = json!("SECRET");
    let error = updates::parse(
        Path::new("/fixture"),
        records(jsonl(&[value]).as_bytes(), "updates.jsonl").unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("invalid timestamp"));
    assert!(!error.contains("SECRET"));
}

#[test]
fn malformed_envelope_unknown_extension_and_mixed_sessions_fail() {
    for value in [
        json!({}),
        json!({"method":"other","params":{}}),
        extension(json!({"sessionUpdate":"future"})),
        extension(json!({"sessionUpdate":"rewind_marker","target_prompt_index":-1})),
    ] {
        assert!(updates::parse(
            Path::new("/fixture"),
            records(jsonl(&[value]).as_bytes(), "updates.jsonl").unwrap()
        )
        .is_err());
    }
    let mut other = assistant("a");
    other["params"]["sessionId"] = json!("another-session");
    assert!(updates::parse(
        Path::new("/fixture"),
        records(jsonl(&[user("u", 0), other]).as_bytes(), "updates.jsonl").unwrap()
    )
    .unwrap_err()
    .contains("mixed session"));
}

#[test]
fn known_chrome_does_not_become_chat_bubbles() {
    assert!(parse(&[
        event(json!({"sessionUpdate":"available_commands_update","availableCommands":[]})),
        extension(json!({"sessionUpdate":"compaction_checkpoint","checkpoint_id":"c"})),
        extension(
            json!({"sessionUpdate":"session_summary_generated","session_summary":"not a bubble"})
        ),
    ])
    .is_empty());
}

#[test]
fn older_chat_history_uses_existing_sanitizer_with_stable_rows_and_tools() {
    let fixture = Fixture::new();
    fixture.write("chat_history.jsonl", concat!(
        "{\"type\":\"user\",\"content\":\"<system-reminder>not a bubble</system-reminder>\"}\n",
        "{\"type\":\"user\",\"synthetic_reason\":\"project_instructions\",\"content\":\"not a bubble\"}\n",
        "{\"type\":\"user\",\"content\":\"again\\n@/fixture/a.png\",\"created_at\":\"2026-01-01T00:00:00Z\"}\n",
        "{\"type\":\"assistant\",\"content\":\"answer\"}\n",
        "{\"type\":\"tool_result\",\"tool_call_id\":\"t\",\"content\":\"result\"}\n",
        "{\"type\":\"user\",\"content\":\"again\"}\n"
    ));
    let rows = fixture.read().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].content, "again");
    assert_eq!(
        rows[0].attachments.as_ref().unwrap()[0].path,
        "/fixture/a.png"
    );
    assert_eq!(rows[0].created_at.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    assert!(rows[2].content.starts_with("tool_step|completed|tool|t"));
    assert_ne!(rows[0].id, rows[3].id);
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        serde_json::to_value(fixture.read().unwrap()).unwrap()
    );
}

#[test]
fn sparse_tool_metadata_does_not_erase_the_start_identity() {
    let rows = parse(&[
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Read","_meta":{"x.ai/tool":{"name":"read_file","label":"Read"}}}),
        ),
        event(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed","_meta":{"timing":42}}),
        ),
    ]);
    assert_eq!(rows[0].content, "tool_step|completed|read_file|Read");
}

#[test]
fn typed_audio_diff_and_terminal_references_are_supported() {
    let rows = parse(&[
        event(
            json!({"sessionUpdate":"user_message_chunk","content":{"type":"audio","mimeType":"audio/wav","data":"aGVsbG8="}}),
        ),
        event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Edit","content":[{"type":"diff","path":"/fixture/a.txt","oldText":"before","newText":"after"},{"type":"terminal","terminalId":"not-transcript-output"}]}),
        ),
    ]);
    let attachment = &rows[0].attachments.as_ref().unwrap()[0];
    assert_eq!(attachment.path, "data:audio/wav;base64,aGVsbG8=");
    assert_eq!(attachment.name, "inline.wav");
    assert!(rows[1].content.contains("/fixture/a.txt\nbefore\nafter"));
    assert!(!rows[1].content.contains("not-transcript-output"));
}

#[test]
fn explicit_hidden_flags_override_user_query_in_both_formats() {
    let mut visible = user("<user_query>visible</user_query>", 0);
    visible["params"]["update"]["synthetic_reason"] = json!("project_instructions");
    let mut hidden = user("<user_query>hidden</user_query>", 1);
    hidden["params"]["_meta"] = json!({"hideFromScrollback":true});
    assert_eq!(parse(&[visible, hidden]).len(), 1);
    let fixture = Fixture::new();
    fixture.write("chat_history.jsonl", "{\"type\":\"user\",\"_meta\":{\"hideFromScrollback\":true},\"content\":\"<user_query>hidden</user_query>\"}\n");
    assert!(fixture.read().unwrap().is_empty());
}

#[test]
fn sequence_only_native_replay_fixtures_are_not_transcripts() {
    // Native extensions/session_updates.rs uses these envelopes for cursor tests.
    let fixture = Fixture::new();
    fixture.write(
        "updates.jsonl",
        concat!(
            "{\"timestamp\":1,\"method\":\"session/update\",\"params\":{\"seq\":1}}\n",
            "{\"timestamp\":2,\"method\":\"session/update\",\"params\":{\"seq\":2}}\n"
        ),
    );
    fixture.write(
        "chat_history.jsonl",
        "{\"type\":\"user\",\"content\":\"not a fallback\"}\n",
    );
    assert_eq!(
        fixture.read().unwrap_err(),
        "CLI transcript line 1: missing or invalid sessionId"
    );
}

#[test]
fn malformed_later_chunk_timestamp_and_tool_status_are_errors() {
    let mut chunk = assistant("second");
    chunk["params"]["_meta"] = json!({"agentTimestampMs":"SECRET"});
    for events in [
        vec![assistant("first"), chunk],
        vec![event(
            json!({"sessionUpdate":"tool_call","toolCallId":"t","status":42}),
        )],
    ] {
        let error = updates::parse(
            Path::new("/fixture"),
            records(jsonl(&events).as_bytes(), "updates.jsonl").unwrap(),
        )
        .unwrap_err();
        assert!(!error.contains("SECRET"));
    }
}
