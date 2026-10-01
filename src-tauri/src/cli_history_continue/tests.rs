use super::*;

#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;

#[test]
fn malformed_nested_native_context_is_rejected_before_cli_can_drop_it() {
    let valid = json!({"type":"assistant", "content":"", "tool_calls":[
        {"id":"call", "name":"read_file", "arguments":"{\"path\":\"file\"}"}
    ]});
    assert!(validate_chat(&serde_json::to_vec(&valid).unwrap(), 1).is_ok());
    for key in ["id", "name", "arguments"] {
        let mut malformed = valid.clone();
        malformed["tool_calls"][0][key] = json!(42);
        assert!(validate_chat(&serde_json::to_vec(&malformed).unwrap(), 1).is_err());
    }
    for malformed in [
        json!({"type":"assistant", "content":"ok", "tool_calls":null}),
        json!({"type":"assistant", "content":"ok", "model_id":42}),
        json!({"type":"assistant", "content":"ok", "reasoning_effort":"invalid"}),
        json!({"type":"user", "content":[], "prompt_index":"0"}),
        json!({"type":"tool_result", "tool_call_id":"call", "content":"ok", "images":[{"type":"image", "url":42}]}),
    ] {
        assert!(validate_chat(&serde_json::to_vec(&malformed).unwrap(), 1).is_err());
    }
}

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
    origin: CliSessionSource,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("cli-continue-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("terminal/sessions/%2Fwork/source");
        let home = root.join("app-home");
        let cwd = PathBuf::from("/work");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&home).unwrap();
        let origin: CliSessionSource = serde_json::from_value(json!({
            "sourceHome": root.join("terminal"), "relativeDir": "sessions/%2Fwork/source",
            "agentSessionId": "source", "cwd": "/work", "title": "Fixture", "revision": "1", "appOwned": false,
        })).unwrap();
        let fixture = Self {
            root,
            source,
            home,
            cwd,
            origin,
        };
        fixture.put("summary.json", &serde_json::to_vec(&json!({
            "info": { "id": "source", "cwd": "/work" }, "chat_format_version": 1,
            "session_summary": "Native fixture", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "num_messages": 3, "num_chat_messages": 5,
            "current_model_id": "old-provider", "agent_id": "old-agent", "attempt_id": "old-attempt",
        })).unwrap());
        // CLI ConversationItem's internally tagged v1 format, not UI messages.
        fixture.put("chat_history.jsonl", concat!(
            "{\"type\":\"system\",\"content\":\"Full system prompt\"}\n",
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Inspect source\"}]}\n",
            "{\"type\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"id\":\"call-1\",\"name\":\"read_file\",\"arguments\":\"{}\"}]}\n",
            "{\"type\":\"tool_result\",\"tool_call_id\":\"call-1\",\"content\":\"Complete file contents\",\"images\":[{\"type\":\"image\",\"url\":\"data:image/png;base64,AA==\"}]}\n",
            "{\"type\":\"assistant\",\"content\":\"Analysis completed\",\"tool_calls\":[]}\n",
        ).as_bytes());
        fixture.put("updates.jsonl", concat!(
            "{\"timestamp\":1,\"method\":\"session/update\",\"params\":{\"sessionId\":\"source\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"source must remain literal\"}}}}\n",
            "{\"timestamp\":2,\"method\":\"_x.ai/session/update\",\"params\":{\"sessionId\":\"source\",\"update\":{\"sessionUpdate\":\"compaction_checkpoint\",\"checkpoint_id\":\"check\",\"prompt_index_at_compaction\":1,\"checkpoint_file\":\"compaction_checkpoints/check.json\",\"schema_version\":1,\"created_at\":\"2026-01-01T00:00:00Z\"}}}\n",
        ).as_bytes());
        fixture
    }
    fn put(&self, relative: &str, bytes: &[u8]) {
        let path = self.source.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn fork(&self) -> std::io::Result<PreparedContinuation> {
        fork_state(
            &self.source,
            "source",
            &self.origin,
            "app-id",
            &self.home,
            &self.cwd,
            "custom-provider",
        )
    }
    fn meta(&self, id: &str) -> SessionMeta {
        let mut origin = self.origin.clone();
        origin.app_owned = true;
        serde_json::from_value(json!({ "id": "app-id", "title": "Fixture", "agentSessionId": id,
            "cliSource": origin, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z" })).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct StoreFixture {
    native: Fixture,
    previous_app_home: Option<std::ffi::OsString>,
    previous_cli_home: Option<std::ffi::OsString>,
    _environment: std::sync::MutexGuard<'static, ()>,
}

impl StoreFixture {
    fn new() -> Self {
        let environment = crate::paths::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut native = Fixture::new();
        let previous_app_home = std::env::var_os("SUPERCHARGE_APP_HOME");
        let previous_cli_home = std::env::var_os("SUPERCHARGE_HOME");
        std::env::set_var("SUPERCHARGE_APP_HOME", native.root.join("store"));
        std::env::set_var("SUPERCHARGE_HOME", native.root.join("terminal"));
        crate::paths::ensure_app_dirs().unwrap();
        native.home = fs::canonicalize(crate::paths::agent_home_dir()).unwrap();
        native.source = fs::canonicalize(&native.source).unwrap();
        let home = fs::canonicalize(native.root.join("terminal")).unwrap();
        native.origin.source_home = home.to_string_lossy().into_owned();
        native.origin.relative_dir = native
            .source
            .strip_prefix(home)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        Self {
            native,
            previous_app_home,
            previous_cli_home,
            _environment: environment,
        }
    }

    fn source_meta(&self) -> SessionMeta {
        let mut meta = crate::store::create_session(None, Some("Terminal".into()), false).unwrap();
        // Discovery creates metadata without an App-owned journal.
        fs::remove_file(crate::paths::session_dir(&meta.id).join("messages.json")).unwrap();
        meta.cli_source = Some(self.native.origin.clone());
        crate::store::update_session_meta(&meta).unwrap();
        meta
    }

    fn prepare(&self, meta: &SessionMeta) -> PreparedContinuation {
        prepare(meta, &self.native.home, &self.native.cwd, "custom-provider").unwrap()
    }
}

impl Drop for StoreFixture {
    fn drop(&mut self) {
        for (key, previous) in [
            ("SUPERCHARGE_APP_HOME", &self.previous_app_home),
            ("SUPERCHARGE_HOME", &self.previous_cli_home),
        ] {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn put_user_turns(fixture: &Fixture) {
    fixture.put("updates.jsonl", &user_turns());
    let history = [
        json!({"type":"system","content":"fixture"}),
        json!({"type":"user","content":[{"type":"text","text":"first question"}],"prompt_index":0}),
        json!({"type":"assistant","content":"first answer"}),
        json!({"type":"user","content":[{"type":"text","text":"second question"}],"prompt_index":1}),
        json!({"type":"assistant","content":"second answer"}),
    ];
    fixture.put(
        "chat_history.jsonl",
        history
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            .as_bytes(),
    );
}

fn user_turns() -> Vec<u8> {
    let mut bytes = Vec::new();
    for (kind, text) in [
        ("user_message_chunk", "first question"),
        ("agent_message_chunk", "first answer"),
        ("user_message_chunk", "second question"),
        ("agent_message_chunk", "second answer"),
    ] {
        serde_json::to_writer(
            &mut bytes,
            &json!({
                "sessionId": "source", "update": {
                    "sessionUpdate": kind, "content": {"type": "text", "text": text}
                }
            }),
        )
        .unwrap();
        bytes.push(b'\n');
    }
    bytes
}

#[test]
fn preparation_lock_is_per_app_session_and_released_on_drop() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let held = lock_preparation(&meta.id).unwrap();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                assert!(lock_preparation(&meta.id).is_err());
                assert!(lock_preparation("independent-app-session").is_ok());
            })
            .join()
            .unwrap();
    });
    crate::store::update_sessions_index(|_| Ok(())).unwrap();
    drop(held);
    let retry = lock_preparation(&meta.id).unwrap();
    assert!(lock_preparation(&meta.id).is_err());
    drop(retry);
    assert!(lock_preparation(&meta.id).is_ok());
}

#[test]
fn superseded_copy_cannot_commit_execution_or_claim_source() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let f = StoreFixture::new();
    let meta = f.source_meta();
    let generation = AtomicU64::new(7);
    let held = lock_preparation(&meta.id).unwrap();
    let prepared = f.prepare(&meta);
    generation.store(8, Ordering::SeqCst);
    let result = commit_if_current(&generation, 7, || {
        record_execution(&meta, &prepared)?;
        crate::cli_history::materialize_execution(&meta.id, &prepared.directory)
    });
    assert!(result.unwrap_err().contains("superseded or timed out"));
    let directory = crate::paths::session_dir(&meta.id);
    assert!(!directory.join("cli_execution.json").exists());
    assert!(!directory.join("messages.json").exists());
    let unchanged = crate::store::load_sessions_index()
        .into_iter()
        .find(|row| row.id == meta.id)
        .unwrap();
    assert!(unchanged.agent_session_id.is_none());
    assert!(!unchanged.cli_source.unwrap().app_owned);
    assert!(lock_preparation(&meta.id).is_err());
    drop(held);
    let _retry = lock_preparation(&meta.id).unwrap();
    commit_if_current(&generation, 8, || record_execution(&meta, &prepared)).unwrap();
    assert!(directory.join("cli_execution.json").exists());
}

#[test]
fn execution_materialization_uses_copied_transcript_not_source_or_cache() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let meta = f.source_meta();
    crate::cli_history::read_messages(&meta.id).unwrap();
    let cache_path = crate::paths::session_dir(&meta.id).join("cli_transcript.json");
    let cached = fs::read(&cache_path).unwrap();
    let prepared = f.prepare(&meta);
    let expected = crate::cli_history_transcript::read_transcript(&prepared.directory).unwrap();
    f.native
        .put("updates.jsonl", b"terminal is rewriting its transcript\n");
    record_execution(&meta, &prepared).unwrap();
    crate::cli_history::materialize_execution(&meta.id, &prepared.directory).unwrap();
    let stored = crate::store::load_messages(&meta.id);
    assert_eq!(
        serde_json::to_value(&stored).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(stored.len(), 4);
    assert_eq!(stored[0].content, "first question");
    assert_eq!(stored[3].content, "second answer");
    assert_eq!(fs::read(cache_path).unwrap(), cached);
    assert_eq!(
        fs::read(f.native.source.join("updates.jsonl")).unwrap(),
        b"terminal is rewriting its transcript\n"
    );
    let owned = crate::store::load_sessions_index()
        .into_iter()
        .find(|row| row.id == meta.id)
        .unwrap();
    assert!(owned.cli_source.unwrap().app_owned);
}

#[test]
fn execution_materialization_preserves_existing_and_empty_journals() {
    let f = StoreFixture::new();
    for empty in [false, true] {
        let meta = f.source_meta();
        let messages = if empty {
            Vec::new()
        } else {
            crate::cli_history_transcript::read_transcript(&f.native.source).unwrap()
        };
        crate::store::replace_messages(&meta.id, &messages).unwrap();
        let journal = crate::paths::session_dir(&meta.id).join("messages.json");
        let before = fs::read(&journal).unwrap();
        crate::cli_history::materialize_execution(&meta.id, &f.native.root.join("missing"))
            .unwrap();
        assert_eq!(fs::read(journal).unwrap(), before);
    }
}

#[test]
fn invalid_execution_transcript_does_not_materialize_or_claim_source() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let prepared = f.prepare(&meta);
    fs::write(
        prepared.directory.join("updates.jsonl"),
        b"invalid complete record\n",
    )
    .unwrap();
    assert!(crate::cli_history::materialize_execution(&meta.id, &prepared.directory).is_err());
    assert!(!crate::paths::session_dir(&meta.id)
        .join("messages.json")
        .exists());
    let unchanged = crate::store::load_sessions_index()
        .into_iter()
        .find(|row| row.id == meta.id)
        .unwrap();
    assert!(!unchanged.cli_source.unwrap().app_owned);
}

#[test]
fn native_forks_of_source_only_history_keep_provenance_and_cut_journal() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let source = f.source_meta();
    let source_before = snapshot(&f.native.source).unwrap();
    for cut in [None, Some(0)] {
        let child = crate::store::fork_session(&source.id, cut, None, true).unwrap();
        assert!(child.fork_agent_session);
        assert_eq!(child.fork_rewind_prompt_index, cut);
        assert_eq!(child.agent_session_id.as_deref(), Some("source"));
        let provenance = child.cli_source.as_ref().unwrap();
        assert_eq!(provenance.source_home, f.native.origin.source_home);
        assert_eq!(provenance.relative_dir, f.native.origin.relative_dir);
        assert!(provenance.app_owned);
        assert_eq!(history_directory(&child), Some(f.native.source.clone()));
        let journal = crate::paths::session_dir(&child.id).join("messages.json");
        let journal_before = fs::read(&journal).unwrap();
        assert_eq!(
            crate::store::load_messages(&child.id).len(),
            if cut.is_some() { 2 } else { 4 }
        );
        let prepared = f.prepare(&child);
        assert_ne!(prepared.agent_session_id, "source");
        crate::cli_history::materialize_execution(&child.id, &prepared.directory).unwrap();
        assert_eq!(fs::read(journal).unwrap(), journal_before);
    }
    assert_eq!(snapshot(&f.native.source).unwrap(), source_before);
    assert!(!crate::paths::session_dir(&source.id)
        .join("messages.json")
        .exists());
    let found = crate::cli_history::discover(&[
        fs::canonicalize(f.native.root.join("terminal")).unwrap(),
        f.native.home.clone(),
    ]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source.agent_session_id, "source");
}

#[test]
fn recorded_execution_retry_reuses_native_copy_and_keeps_terminal_separate() {
    let f = StoreFixture::new();
    let mut meta = f.source_meta();
    let prepared = f.prepare(&meta);
    record_execution(&meta, &prepared).unwrap();
    crate::cli_history::materialize_execution(&meta.id, &prepared.directory).unwrap();
    meta.agent_session_id = Some(prepared.agent_session_id.clone());
    meta.cli_source.as_mut().unwrap().app_owned = true;
    crate::store::update_session_meta(&meta).unwrap();
    let execution_id = prepared.agent_session_id.clone();
    let execution_directory = prepared.directory.clone();
    drop(prepared);
    let retry = f.prepare(&meta);
    assert_eq!(retry.agent_session_id, execution_id);
    assert_eq!(retry.directory, execution_directory);
    let found = crate::cli_history::discover(&[
        fs::canonicalize(f.native.root.join("terminal")).unwrap(),
        f.native.home.clone(),
    ]);
    assert_eq!(found.len(), 1);
    let mut index = crate::store::load_sessions_index();
    assert!(crate::cli_history::merge_discovered(
        &mut index,
        &found,
        &Default::default()
    ));
    assert_eq!(index.len(), 2);
    assert_eq!(index.iter().filter(|row| row.id == meta.id).count(), 1);
    assert!(index.iter().any(|row| row.id != meta.id
        && row
            .cli_source
            .as_ref()
            .is_some_and(|source| !source.app_owned)));
    assert!(!crate::cli_history::merge_discovered(
        &mut index,
        &found,
        &Default::default()
    ));
}

#[test]
fn failed_native_forks_preserve_pending_cuts_and_reuse_child_execution() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let source = f.source_meta();
    for cut in [None, Some(0)] {
        let mut child = crate::store::fork_session(&source.id, cut, None, true).unwrap();
        let prepared = f.prepare(&child);
        record_execution(&child, &prepared).unwrap();
        crate::cli_history::materialize_execution(&child.id, &prepared.directory).unwrap();
        child.agent_session_id = Some(prepared.agent_session_id.clone());
        crate::store::update_session_meta(&child).unwrap();
        let execution_id = prepared.agent_session_id.clone();
        let execution_directory = prepared.directory.clone();
        let journal = crate::paths::session_dir(&child.id).join("messages.json");
        let journal_before = fs::read(&journal).unwrap();
        drop(prepared);

        for _ in 0..2 {
            clear_failed_legacy_fork(&child).unwrap();
            let retry_meta = crate::store::load_sessions_index()
                .into_iter()
                .find(|row| row.id == child.id)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&retry_meta).unwrap(),
                serde_json::to_value(&child).unwrap()
            );
            assert!(retry_meta.fork_agent_session);
            assert_eq!(retry_meta.fork_rewind_prompt_index, cut);
            let retry = f.prepare(&retry_meta);
            assert_eq!(retry.agent_session_id, execution_id);
            assert_eq!(retry.directory, execution_directory);
            assert_eq!(fs::read(&journal).unwrap(), journal_before);
        }
    }
}

#[test]
fn pending_fork_rejects_recorded_execution_owned_by_another_app() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let source = f.source_meta();
    let mut child = crate::store::fork_session(&source.id, Some(0), None, true).unwrap();
    let prepared = f.prepare(&child);
    record_execution(&child, &prepared).unwrap();
    child.agent_session_id = Some(prepared.agent_session_id.clone());
    crate::store::update_session_meta(&child).unwrap();
    let marker_path = prepared.directory.join(ORIGIN_FILE);
    let parent = prepared.directory.parent().unwrap().to_owned();
    drop(prepared);
    let mut marker: Value = serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    marker["appSessionId"] = json!("another-app-session");
    fs::write(marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
    let entries_before = fs::read_dir(&parent).unwrap().count();
    let error = prepare(&child, &f.native.home, &f.native.cwd, "custom-provider").unwrap_err();
    assert!(error.contains("belongs to another App session"));
    assert_eq!(fs::read_dir(parent).unwrap().count(), entries_before);
}

#[test]
fn failed_legacy_forks_still_clear_one_shot_and_partial_agent_identity() {
    let f = StoreFixture::new();
    for cut in [None, Some(0)] {
        let mut meta = f.source_meta();
        meta.cli_source = None;
        meta.agent_session_id = Some("legacy-parent".into());
        meta.fork_agent_session = true;
        meta.fork_rewind_prompt_index = cut;
        crate::store::update_session_meta(&meta).unwrap();
        clear_failed_legacy_fork(&meta).unwrap();
        let saved = crate::store::load_sessions_index()
            .into_iter()
            .find(|row| row.id == meta.id)
            .unwrap();
        assert!(!saved.fork_agent_session);
        assert_eq!(saved.fork_rewind_prompt_index, None);
        assert_eq!(
            saved.agent_session_id.as_deref(),
            if cut.is_some() {
                None
            } else {
                Some("legacy-parent")
            }
        );
    }
}

#[test]
fn matching_agent_id_resolves_recorded_source_despite_duplicate_home_id() {
    let f = StoreFixture::new();
    let mut meta = f.source_meta();
    meta.agent_session_id = Some("source".into());
    meta.cli_source.as_mut().unwrap().app_owned = true;
    let duplicate = f
        .native
        .home
        .join("sessions")
        .join("other-cwd")
        .join("source");
    fs::create_dir_all(&duplicate).unwrap();
    fs::copy(
        f.native.source.join("summary.json"),
        duplicate.join("summary.json"),
    )
    .unwrap();
    assert_eq!(history_directory(&meta), Some(f.native.source.clone()));
    let provenance = fork_source(&meta).unwrap().unwrap();
    assert_eq!(provenance.source_home, f.native.origin.source_home);
    assert_eq!(provenance.relative_dir, f.native.origin.relative_dir);
}

#[test]
fn native_fork_of_owned_history_copies_latest_execution_not_terminal() {
    let f = StoreFixture::new();
    let mut parent = f.source_meta();
    let prepared = f.prepare(&parent);
    record_execution(&parent, &prepared).unwrap();
    crate::cli_history::materialize_execution(&parent.id, &prepared.directory).unwrap();
    parent.agent_session_id = Some(prepared.agent_session_id.clone());
    parent.cli_source.as_mut().unwrap().app_owned = true;
    crate::store::update_session_meta(&parent).unwrap();
    let mut chat = OpenOptions::new()
        .append(true)
        .open(prepared.directory.join("chat_history.jsonl"))
        .unwrap();
    chat.write_all(b"{\"type\":\"assistant\",\"content\":\"App-only answer\",\"tool_calls\":[]}\n")
        .unwrap();
    drop(chat);
    let child = crate::store::fork_session(&parent.id, None, None, true).unwrap();
    assert_eq!(child.agent_session_id, parent.agent_session_id);
    let provenance = child.cli_source.as_ref().unwrap();
    assert_eq!(provenance.agent_session_id, prepared.agent_session_id);
    assert_eq!(history_directory(&child), Some(prepared.directory.clone()));
    let forked = f.prepare(&child);
    assert_ne!(forked.agent_session_id, prepared.agent_session_id);
    assert_eq!(
        fs::read(forked.directory.join("chat_history.jsonl")).unwrap(),
        fs::read(prepared.directory.join("chat_history.jsonl")).unwrap()
    );
    assert_ne!(
        fs::read(forked.directory.join("chat_history.jsonl")).unwrap(),
        fs::read(f.native.source.join("chat_history.jsonl")).unwrap()
    );
}

#[test]
fn native_v1_context_and_sidecars_survive_without_credentials_or_live_state() {
    let f = Fixture::new();
    for (path, data) in [
        ("plan.json", "{\"items\":[]}"),
        (
            "plan_mode.json",
            "{\"state\":\"plan\",\"awaiting_plan_approval\":true}",
        ),
        ("plan.md", "Detailed plan"),
        ("tool_state.json", "{}"),
        ("signals.json", "{}"),
        (
            "compaction/segment_0001.md",
            "Pre-compaction full transcript",
        ),
        ("compaction_checkpoints/check.json", "{\"checkpoint_id\":\"check\",\"prompt_index_at_compaction\":1,\"compacted_history\":[],\"schema_version\":1}"),
        ("images/1.png", "image"),
        ("usage.json", "{\"session_id\":\"source\",\"turns\":[]}"),
        ("title_refresh_idx", "2"),
        ("auth.json", "secret"),
        ("config.toml", "secret"),
        ("summary.json.lock", "live"),
        ("workflows/run.json", "live orchestration"),
        ("goal/state.json", "live goal"),
    ] {
        f.put(path, data.as_bytes());
    }
    let before = snapshot(&f.source).unwrap();
    let prepared = f.fork().unwrap();
    assert_eq!(before, snapshot(&f.source).unwrap());
    assert_eq!(
        fs::read(prepared.directory.join("chat_history.jsonl")).unwrap(),
        before[Path::new("chat_history.jsonl")]
    );
    for path in [
        "plan.json",
        "plan_mode.json",
        "plan.md",
        "tool_state.json",
        "signals.json",
        "compaction/segment_0001.md",
        "compaction_checkpoints/check.json",
        "images/1.png",
        "title_refresh_idx",
    ] {
        assert_eq!(
            fs::read(prepared.directory.join(path)).unwrap(),
            before[Path::new(path)]
        );
    }
    for path in [
        "auth.json",
        "config.toml",
        "summary.json.lock",
        "workflows",
        "goal",
    ] {
        assert!(!prepared.directory.join(path).exists());
    }
    let summary: Value =
        serde_json::from_slice(&fs::read(prepared.directory.join("summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["info"]["id"], prepared.agent_session_id);
    assert_eq!(summary["current_model_id"], "custom-provider");
    assert_eq!(summary["parent_session_id"], "source");
    assert!(summary.get("agent_id").is_none());
    let updates = fs::read_to_string(prepared.directory.join("updates.jsonl")).unwrap();
    for line in updates.lines() {
        assert_eq!(
            serde_json::from_str::<Value>(line).unwrap()["params"]["sessionId"],
            prepared.agent_session_id
        );
    }
    assert!(updates.contains("source must remain literal"));
    let marker: Value =
        serde_json::from_slice(&fs::read(prepared.directory.join(ORIGIN_FILE)).unwrap()).unwrap();
    assert_eq!(marker["origin"]["agentSessionId"], "source");
    assert_eq!(marker["sourceDigest"], digest(&before));
}

#[test]
fn same_home_origin_is_still_isolated_and_reconnect_reuses_execution() {
    let mut f = Fixture::new();
    f.home = f.root.join("terminal");
    let child = f.fork().unwrap();
    assert_ne!(child.directory, f.source);
    let meta = f.meta(&child.agent_session_id);
    let reused = prepare_directory(
        &meta,
        &f.home,
        &f.cwd,
        "other-custom",
        child.directory.clone(),
        &child.agent_session_id,
        true,
    )
    .unwrap();
    assert_eq!(reused.agent_session_id, child.agent_session_id);
    assert!(prepare_directory(
        &meta,
        &f.home,
        &f.cwd,
        "other-custom",
        child.directory.clone(),
        &child.agent_session_id,
        true
    )
    .is_err());
    drop(reused);
    assert!(prepare_directory(
        &meta,
        &f.home,
        &f.cwd,
        "other-custom",
        child.directory.clone(),
        &child.agent_session_id,
        true
    )
    .is_ok());
}

#[test]
fn home_change_copies_latest_execution_not_diverged_origin() {
    let f = Fixture::new();
    let first = f.fork().unwrap();
    let mut chat = OpenOptions::new()
        .append(true)
        .open(first.directory.join("chat_history.jsonl"))
        .unwrap();
    chat.write_all(
        b"{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"App-only followup\"}]}\n",
    )
    .unwrap();
    drop(chat);
    let next_home = f.root.join("next-home");
    let meta = f.meta(&first.agent_session_id);
    let next = prepare_directory(
        &meta,
        &next_home,
        &f.cwd,
        "custom-next",
        first.directory.clone(),
        &first.agent_session_id,
        true,
    )
    .unwrap();
    assert_ne!(next.agent_session_id, first.agent_session_id);
    let bytes = fs::read(next.directory.join("chat_history.jsonl")).unwrap();
    assert_eq!(
        bytes,
        fs::read(first.directory.join("chat_history.jsonl")).unwrap()
    );
    assert_ne!(
        bytes,
        fs::read(f.source.join("chat_history.jsonl")).unwrap()
    );
    let origin: Value =
        serde_json::from_slice(&fs::read(next.directory.join(ORIGIN_FILE)).unwrap()).unwrap();
    assert_eq!(origin["origin"]["agentSessionId"], "source");
}

#[test]
fn long_cwd_preserves_cli_hash_directory_and_marker() {
    let mut f = Fixture::new();
    let long_cwd = format!("/work/{}", "project-".repeat(50));
    let hash_dir = f.root.join("terminal/sessions/project-0123456789abcdef");
    fs::rename(f.source.parent().unwrap(), &hash_dir).unwrap();
    f.source = hash_dir.join("source");
    fs::write(hash_dir.join(".cwd"), long_cwd.as_bytes()).unwrap();
    let mut summary: Value =
        serde_json::from_slice(&fs::read(f.source.join("summary.json")).unwrap()).unwrap();
    summary["info"]["cwd"] = json!(long_cwd);
    f.put("summary.json", &serde_json::to_vec(&summary).unwrap());
    f.cwd = PathBuf::from(&long_cwd);
    let child = f.fork().unwrap();
    assert_eq!(
        child.directory.parent().unwrap().file_name().unwrap(),
        "project-0123456789abcdef"
    );
    assert_eq!(
        fs::read_to_string(child.directory.parent().unwrap().join(".cwd")).unwrap(),
        long_cwd
    );
}

#[test]
fn valid_json_with_unknown_native_item_is_not_silently_dropped() {
    let f = Fixture::new();
    f.put(
        "chat_history.jsonl",
        b"{\"type\":\"future_context_item\",\"content\":\"must not lose\"}\n",
    );
    assert!(f.fork().is_err());
}

#[test]
fn terminal_registration_blocks_loading_app_execution_in_place() {
    let f = Fixture::new();
    let child = f.fork().unwrap();
    fs::write(f.home.join("active_sessions.json"), serde_json::to_vec(&json!([{
        "session_id": child.agent_session_id, "pid": std::process::id(), "cwd": "/work", "opened_at": "2026-01-01T00:00:00Z"
    }])).unwrap()).unwrap();
    assert!(prepare_directory(
        &f.meta(&child.agent_session_id),
        &f.home,
        &f.cwd,
        "custom",
        child.directory.clone(),
        &child.agent_session_id,
        true
    )
    .is_err());
}

#[test]
fn corrupt_or_missing_native_context_never_bootstraps() {
    let f = Fixture::new();
    f.put("chat_history.jsonl", b"{\"type\":\"user\"");
    assert!(f.fork().is_err());
    fs::remove_file(f.source.join("chat_history.jsonl")).unwrap();
    assert!(f.fork().is_err());
    assert!(!f.home.join("sessions").exists());
}

#[test]
fn future_formats_and_changed_cwd_are_rejected() {
    let mut f = Fixture::new();
    f.cwd = PathBuf::from("/elsewhere");
    assert!(f.fork().is_err());
    f.cwd = PathBuf::from("/work");
    let mut summary: Value =
        serde_json::from_slice(&fs::read(f.source.join("summary.json")).unwrap()).unwrap();
    summary["chat_format_version"] = json!(2);
    f.put("summary.json", &serde_json::to_vec(&summary).unwrap());
    assert!(f.fork().is_err());
}

#[test]
fn legacy_v0_chat_and_raw_updates_remain_compatible() {
    let f = Fixture::new();
    let mut summary: Value =
        serde_json::from_slice(&fs::read(f.source.join("summary.json")).unwrap()).unwrap();
    summary["chat_format_version"] = json!(0);
    f.put("summary.json", &serde_json::to_vec(&summary).unwrap());
    f.put(
        "chat_history.jsonl",
        b"{\"role\":\"user\",\"content\":\"legacy conversation\"}\n",
    );
    f.put("updates.jsonl", b"{\"sessionId\":\"source\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"legacy\"}}}\n");
    let child = f.fork().unwrap();
    assert_eq!(
        fs::read(child.directory.join("chat_history.jsonl")).unwrap(),
        fs::read(f.source.join("chat_history.jsonl")).unwrap()
    );
    let update: Value =
        serde_json::from_slice(&fs::read(child.directory.join("updates.jsonl")).unwrap()).unwrap();
    assert_eq!(update["sessionId"], child.agent_session_id);
}

#[test]
fn atomic_publish_cannot_replace_even_an_empty_existing_directory() {
    let f = Fixture::new();
    let stage = f.root.join("stage");
    let target = f.root.join("existing");
    fs::create_dir(&stage).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(stage.join("sentinel"), b"new").unwrap();
    assert!(publish_no_replace(&stage, &target).is_err());
    assert!(stage.join("sentinel").exists());
    assert!(!target.join("sentinel").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_files_and_directories_fail_closed() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let secret = f.root.join("secret");
    fs::write(&secret, b"secret").unwrap();
    symlink(&secret, f.source.join("plan.md")).unwrap();
    assert!(f.fork().is_err());
    fs::remove_file(f.source.join("plan.md")).unwrap();
    symlink(&f.root, f.source.join("compaction")).unwrap();
    assert!(f.fork().is_err());
    assert!(!f.home.join("sessions").exists());
}

#[test]
fn oversized_state_is_rejected_without_truncation() {
    let f = Fixture::new();
    let file = fs::File::create(f.source.join("chat_history.jsonl")).unwrap();
    file.set_len(MAX_BYTES + 1).unwrap();
    assert!(f.fork().is_err());
    assert!(!f.home.join("sessions").exists());
}
