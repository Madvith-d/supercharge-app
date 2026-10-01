use super::*;

#[test]
fn startup_system_rows_do_not_count_as_a_terminal_conversation() {
    assert!(!visible_summary(&serde_json::json!({
        "session_summary":"", "num_messages":0, "num_chat_messages":2
    })));
    assert!(visible_summary(&serde_json::json!({
        "session_summary":"", "num_messages":1, "num_chat_messages":3
    })));
}

struct AppFixture {
    _fixture: Fixture,
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl AppFixture {
    fn new() -> Self {
        let lock = crate::paths::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let fixture = Fixture::new();
        let previous = std::env::var_os("SUPERCHARGE_APP_HOME");
        std::env::set_var("SUPERCHARGE_APP_HOME", &fixture.0);
        crate::paths::ensure_app_dirs().unwrap();
        Self {
            _fixture: fixture,
            previous,
            _lock: lock,
        }
    }

    fn source(&self) -> SessionMeta {
        let home = crate::paths::agent_home_dir();
        let dir = session(&home, "cwd", "id");
        fs::write(
            dir.join("chat_history.jsonl"),
            b"{\"role\":\"user\",\"content\":\"searchable terminal prompt\"}\n",
        )
        .unwrap();
        let meta = new_meta(&discover(&[home])[0]);
        store::save_sessions_index(std::slice::from_ref(&meta)).unwrap();
        meta
    }
}

impl Drop for AppFixture {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("SUPERCHARGE_APP_HOME", value),
            None => std::env::remove_var("SUPERCHARGE_APP_HOME"),
        }
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cli-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn home(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn session(home: &Path, cwd: &str, id: &str) -> PathBuf {
    let dir = home.join("sessions").join(cwd).join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("summary.json"), br#"{"info":{"cwd":"/missing/workspace"},"generated_title":"Terminal conversation","created_at":"2024-01-02T00:00:00Z","updated_at":"2024-02-03T00:00:00Z"}"#).unwrap();
    dir
}

#[test]
fn discovers_all_three_homes_without_mode_or_transcript_reads_or_caps() {
    let fixture = Fixture::new();
    let default = fixture.home("default");
    let inherited = fixture.home("override");
    let independent = fixture.home("agent-home");
    for home in [&default, &inherited, &independent] {
        fs::write(home.join("config.toml"), b"provider = 'unchanged'\n").unwrap();
        fs::write(home.join("auth.json"), b"unchanged credentials").unwrap();
        for i in 0..205 {
            session(home, "%2Fmissing", &format!("id-{i}"));
        }
    }
    // Invalid transcript contents must never affect metadata discovery.
    fs::write(
        default.join("sessions/%2Fmissing/id-0/updates.jsonl"),
        b"not json",
    )
    .unwrap();
    let roots = source_homes(
        default.clone(),
        Some(inherited.clone()),
        independent.clone(),
    );
    let rows = discover(&roots);
    assert_eq!(rows.len(), 615);
    let mut index = Vec::new();
    assert!(merge_discovered(&mut index, &rows, &BTreeSet::new()));
    assert!(!merge_discovered(&mut index, &rows, &BTreeSet::new()));
    assert_eq!(index.len(), 615);
    assert!(index
        .iter()
        .all(|meta| meta.agent_session_id.is_none() && meta.project_id.is_none()));
    assert!(index
        .iter()
        .all(|meta| meta.created_at.to_rfc3339() == "2024-01-02T00:00:00+00:00"));
    assert_eq!(
        index
            .iter()
            .filter(|meta| meta.cli_source.as_ref().unwrap().agent_session_id == "id-0")
            .count(),
        3
    );
    for home in [&default, &inherited, &independent] {
        assert_eq!(
            fs::read(home.join("config.toml")).unwrap(),
            b"provider = 'unchanged'\n"
        );
        assert_eq!(
            fs::read(home.join("auth.json")).unwrap(),
            b"unchanged credentials"
        );
    }
    assert!(!fixture.0.join("projects.json").exists());
}

#[test]
fn incomplete_summary_recovers_title_and_source_chronology() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    let dir = session(&home, "cwd", "id");
    fs::write(dir.join("summary.json"), b"{").unwrap();
    let mut index = Vec::new();
    merge_discovered(
        &mut index,
        &discover(std::slice::from_ref(&home)),
        &BTreeSet::new(),
    );
    assert!(index.is_empty());
    session(&home, "cwd", "id");
    assert!(merge_discovered(
        &mut index,
        &discover(&[home]),
        &BTreeSet::new()
    ));
    assert_eq!(index[0].title, "Terminal conversation");
    assert_eq!(
        index[0].created_at.to_rfc3339(),
        "2024-01-02T00:00:00+00:00"
    );
}

#[test]
fn native_visibility_and_managed_copies_are_respected() {
    use serde_json::json;
    assert!(!visible_summary(
        &json!({"generated_title":"Child", "session_kind":"subagent_worker"})
    ));
    assert!(visible_summary(
        &json!({"generated_title":"Child", "session_kind":"subagent", "hidden":false})
    ));
    assert!(!visible_summary(
        &json!({"generated_title":"Hidden", "hidden":true})
    ));
    assert!(!visible_summary(&json!({"num_chat_messages":0})));
    assert!(visible_summary(
        &json!({"num_chat_messages":0, "parent_session_id":"parent"})
    ));
    let fixture = Fixture::new();
    let home = fixture.home("home");
    let dir = session(&home, "cwd", "copy");
    fs::write(dir.join("app_cli_origin.json"), b"{}").unwrap();
    assert!(discover(&[home]).is_empty());
}

#[test]
fn continued_app_copy_does_not_hide_terminal_origin_or_reimport_execution() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "original");
    let found = discover(&[home]);
    let mut continued = new_meta(&found[0]);
    continued.agent_session_id = Some("execution".into());
    continued.cli_source.as_mut().unwrap().app_owned = true;
    let mut index = vec![continued];
    assert!(merge_discovered(&mut index, &found, &BTreeSet::new()));
    assert_eq!(index.len(), 2);
    assert!(is_continuation_copy(&index[0]));
    assert!(!is_continuation_copy(&index[1]));
    assert!(!merge_discovered(&mut index, &found, &BTreeSet::new()));
    let summary = legacy::summaries(&found, &index);
    assert_eq!(
        summary[0].app_session_id.as_deref(),
        Some(index[1].id.as_str())
    );
}

#[test]
fn aliases_deduplicate_and_missing_roots_are_read_only() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let missing = fixture.0.join("missing");
    let roots = source_homes(home.clone(), Some(home.join(".")), missing.clone());
    assert_eq!(roots, vec![home]);
    assert_eq!(discover(&roots).len(), 1);
    assert!(!missing.exists());
}

#[test]
fn only_unambiguous_nonfork_legacy_links_are_backfilled() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let found = discover(std::slice::from_ref(&home));
    let mut legacy = new_meta(&found[0]);
    legacy.cli_source = None;
    legacy.agent_session_id = Some("id".into());
    legacy.title = "App title".into();
    legacy.pinned = true;
    legacy.archived = true;
    legacy.project_id = Some("existing-project".into());
    legacy.model_id = Some("custom-model".into());
    let mut index = vec![legacy.clone()];
    assert!(merge_discovered(&mut index, &found, &BTreeSet::new()));
    assert_eq!(index.len(), 1);
    assert!(index[0].cli_source.as_ref().unwrap().app_owned);
    assert_eq!(index[0].title, legacy.title);
    assert_eq!(index[0].project_id, legacy.project_id);
    assert_eq!(index[0].model_id, legacy.model_id);
    assert!(index[0].pinned && index[0].archived);

    let mut fork = legacy.clone();
    fork.id = "fork".into();
    fork.fork_agent_session = true;
    let mut index = vec![legacy.clone(), fork.clone()];
    merge_discovered(&mut index, &found, &BTreeSet::new());
    assert_eq!(index.len(), 3);
    assert!(index[0].cli_source.is_none() && index[1].cli_source.is_none());

    let mut index = vec![fork];
    merge_discovered(&mut index, &found, &BTreeSet::new());
    assert_eq!(index.len(), 2);
    assert!(index[0].cli_source.is_none());

    let second = fixture.home("other");
    session(&second, "cwd", "id");
    let found = discover(&[home, second]);
    let mut index = vec![legacy];
    merge_discovered(&mut index, &found, &BTreeSet::new());
    assert_eq!(index.len(), 3);
    assert!(index[0].cli_source.is_none());
}

#[test]
fn metadata_refresh_preserves_app_fields_and_reports_transcript_changes() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    let dir = session(&home, "cwd", "id");
    let mut index = Vec::new();
    merge_discovered(
        &mut index,
        &discover(std::slice::from_ref(&home)),
        &BTreeSet::new(),
    );
    index[0].title = "Renamed".into();
    index[0].archived = true;
    let id = index[0].id.clone();
    fs::write(dir.join("updates.jsonl"), b"delayed terminal flush").unwrap();
    let found = discover(std::slice::from_ref(&home));
    assert!(merge_discovered(&mut index, &found, &BTreeSet::new()));
    assert!(!merge_discovered(&mut index, &found, &BTreeSet::new()));
    assert_eq!(index[0].id, id);
    assert_eq!(index[0].title, "Renamed");
    assert!(index[0].archived);
    fs::remove_dir_all(&dir).unwrap();
    assert!(!merge_discovered(
        &mut index,
        &discover(&[home]),
        &BTreeSet::new()
    ));
    assert_eq!(index.len(), 1, "missing histories remain recoverable");
}

#[test]
fn suppressions_are_durable_source_specific_and_not_capped() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let found = discover(std::slice::from_ref(&home));
    let path = fixture.0.join("suppressed.json");
    let mut source = found[0].source.clone();
    for i in 0..650 {
        source.relative_dir = format!("sessions/cwd/{i}");
        suppress_source_at(&path, &source).unwrap();
    }
    suppress_source_at(&path, &found[0].source).unwrap();
    assert_eq!(load_suppressions(&path).unwrap().len(), 651);
    let mut index = Vec::new();
    assert!(!merge_discovered(
        &mut index,
        &found,
        &load_suppressions(&path).unwrap()
    ));
    let other = fixture.home("other");
    session(&other, "cwd", "id");
    assert!(merge_discovered(
        &mut index,
        &discover(&[other]),
        &load_suppressions(&path).unwrap()
    ));
    assert_eq!(index.len(), 1);
    assert!(home.join("sessions/cwd/id/summary.json").exists());
    fs::write(&path, b"broken").unwrap();
    assert!(
        load_suppressions(&path).is_err(),
        "corruption must not resurrect suppressed rows"
    );
}

#[test]
fn resolution_rejects_traversal_wrong_home_and_excess_depth() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let roots = vec![home];
    let source = discover(&roots)[0].source.clone();
    assert!(resolve_source_in(&source, &roots).is_ok());
    assert!(resolve_source_in(&source, &[]).is_err());
    for relative in [
        "../sessions/cwd/id",
        "/sessions/cwd/id",
        "sessions/cwd/../id",
        "sessions/cwd/deeper/id",
        "sessions/id",
    ] {
        let mut bad = source.clone();
        bad.relative_dir = relative.into();
        assert!(resolve_source_in(&bad, &roots).is_err(), "{relative}");
    }
    let mut bad = source;
    bad.agent_session_id = "different".into();
    assert!(resolve_source_in(&bad, &roots).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_alias_roots_deduplicate_but_session_and_leaf_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let home = fixture.home("home");
    let dir = session(&home, "cwd", "id");
    let alias = fixture.0.join("alias");
    symlink(&home, &alias).unwrap();
    let roots = source_homes(home.clone(), Some(alias), fixture.0.join("missing"));
    assert_eq!(roots.len(), 1);
    let source = discover(&roots)[0].source.clone();
    let outside = fixture.home("outside");
    fs::write(outside.join("updates.jsonl"), b"private").unwrap();
    symlink(outside.join("updates.jsonl"), dir.join("updates.jsonl")).unwrap();
    assert!(resolve_source_in(&source, &roots).is_err());
    assert!(discover(&roots).is_empty());
    fs::remove_file(dir.join("updates.jsonl")).unwrap();
    fs::rename(&dir, outside.join("id")).unwrap();
    symlink(outside.join("id"), &dir).unwrap();
    assert!(resolve_source_in(&source, &roots).is_err());
    assert!(discover(&roots).is_empty());
}

#[cfg(unix)]
#[test]
fn distinct_case_sensitive_homes_are_not_conflated() {
    let fixture = Fixture::new();
    let a = fixture.home("home");
    let b = fixture.home("HOME");
    if fs::canonicalize(&a).unwrap() == fs::canonicalize(&b).unwrap() {
        return;
    }
    session(&a, "cwd", "same-id");
    session(&b, "cwd", "same-id");
    assert_eq!(
        discover(&source_homes(a, Some(b), fixture.0.join("missing"))).len(),
        2
    );
}

#[test]
fn existing_even_empty_journals_and_forks_always_win_over_sources() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let mut meta = new_meta(&discover(&[home])[0]);
    assert!(!transcript::owns_journal_with(&meta, false));
    assert!(transcript::owns_journal_with(&meta, true));
    meta.cli_source.as_mut().unwrap().app_owned = true;
    assert!(transcript::owns_journal_with(&meta, false));
    meta.cli_source.as_mut().unwrap().app_owned = false;
    meta.fork_agent_session = true;
    assert!(transcript::owns_journal_with(&meta, false));
    meta.fork_agent_session = false;
    meta.fork_rewind_prompt_index = Some(0);
    assert!(transcript::owns_journal_with(&meta, false));
}

#[test]
fn known_metadata_reads_keep_cache_separate_and_revalidate_stale_snapshots() {
    let fixture = AppFixture::new();
    let meta = fixture.source();
    let messages = read_messages_with_meta(&meta).unwrap().unwrap();
    assert_eq!(messages[0].content, "searchable terminal prompt");
    let dir = crate::paths::session_dir(&meta.id);
    assert!(!dir.join("messages.json").exists());
    let cache = fs::read(dir.join("cli_transcript.json")).unwrap();
    assert_eq!(
        read_messages(&meta.id).unwrap().unwrap()[0].content,
        messages[0].content
    );

    let mut current = meta.clone();
    current.cli_source.as_mut().unwrap().relative_dir = "sessions/other/id".into();
    store::save_sessions_index(&[current]).unwrap();
    assert!(read_messages_with_meta(&meta)
        .unwrap_err()
        .contains("source changed"));

    for ownership in ["app", "fork", "rewind"] {
        let mut current = meta.clone();
        match ownership {
            "app" => current.cli_source.as_mut().unwrap().app_owned = true,
            "fork" => current.fork_agent_session = true,
            _ => current.fork_rewind_prompt_index = Some(0),
        }
        store::save_sessions_index(&[current]).unwrap();
        assert!(
            read_messages_with_meta(&meta).unwrap().unwrap().is_empty(),
            "{ownership}"
        );
    }
    store::save_sessions_index(&[]).unwrap();
    assert!(read_messages_with_meta(&meta)
        .unwrap_err()
        .contains("removed"));
    assert_eq!(fs::read(dir.join("cli_transcript.json")).unwrap(), cache);
    assert!(!dir.join("messages.json").exists());
}

#[test]
fn cache_hit_waits_for_index_transaction_and_returns_new_app_journal() {
    let fixture = AppFixture::new();
    let meta = fixture.source();
    let mut live = read_messages_with_meta(&meta).unwrap().unwrap();
    live[0].content = "live App turn".into();
    let journal = crate::paths::session_dir(&meta.id).join("messages.json");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let reader = store::update_sessions_index(|index| {
        let snapshot = meta.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            result_tx.send(read_messages_with_meta(&snapshot)).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            matches!(
                result_rx.recv_timeout(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "cache hits must validate under the index lock"
        );
        crate::store_lock::write_bytes_replace(&journal, &serde_json::to_vec(&live).unwrap())?;
        index[0].cli_source.as_mut().unwrap().app_owned = true;
        Ok(reader)
    })
    .unwrap();
    let messages = result_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .unwrap();
    reader.join().unwrap();
    assert_eq!(messages[0].content, "live App turn");
    assert_eq!(store::load_messages(&meta.id)[0].content, "live App turn");
    fs::write(&journal, b"[]").unwrap();
    assert!(read_messages_with_meta(&meta).unwrap().unwrap().is_empty());
}

#[test]
fn known_non_cli_metadata_does_not_load_or_recover_the_index() {
    let fixture = AppFixture::new();
    let mut meta = fixture.source();
    meta.cli_source = None;
    let index = crate::paths::app_data_root().join("sessions_index.json");
    fs::write(&index, b"invalid index").unwrap();
    assert!(read_messages_with_meta(&meta).unwrap().is_none());
    assert_eq!(fs::read(index).unwrap(), b"invalid index");
}

#[test]
fn content_search_reads_app_journals_and_cli_projections_without_claiming_sources() {
    let fixture = AppFixture::new();
    let source = fixture.source();
    let mut messages = read_messages_with_meta(&source).unwrap().unwrap();
    let mut app = source.clone();
    app.id = uuid::Uuid::new_v4().to_string();
    app.cli_source = None;
    messages[0].content = "searchable App prompt".into();
    let dir = crate::paths::session_dir(&app.id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("messages.json"),
        serde_json::to_vec(&messages).unwrap(),
    )
    .unwrap();
    store::save_sessions_index(&[source.clone(), app.clone()]).unwrap();
    let hits = crate::session_content_search::search_sessions("searchable", 10);
    assert_eq!(hits.len(), 2);
    assert!(hits
        .iter()
        .any(|hit| hit.id == app.id && hit.snippet == "searchable App prompt"));
    assert!(hits
        .iter()
        .any(|hit| hit.id == source.id && hit.snippet == "searchable terminal prompt"));
    assert!(!crate::paths::session_dir(&source.id)
        .join("messages.json")
        .exists());
    app.archived = true;
    store::save_sessions_index(&[source, app]).unwrap();
    assert_eq!(
        crate::session_content_search::search_sessions("searchable", 10).len(),
        1
    );
}

#[test]
fn oversized_summaries_remain_excluded_without_reading_histories() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    let dir = session(&home, "cwd", "id");
    let summary = serde_json::json!({ "generated_title": "x".repeat(SUMMARY_LIMIT as usize) });
    fs::write(
        dir.join("summary.json"),
        serde_json::to_vec(&summary).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.join("chat_history.jsonl"),
        b"{\"role\":\"user\",\"content\":\"must not unhide\"}\n",
    )
    .unwrap();
    let found = discover(&[home]);
    assert!(legacy::summaries(&found, &[]).is_empty());
}

#[test]
fn concurrent_index_transactions_link_each_source_once() {
    let fixture = Fixture::new();
    let home = fixture.home("home");
    session(&home, "cwd", "id");
    let found = std::sync::Arc::new(discover(&[home]));
    let path = fixture.0.join("index.json");
    fs::write(&path, b"[]").unwrap();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let found = found.clone();
            std::thread::spawn(move || {
                crate::store_lock::with_exclusive_lock(&path, || {
                    let mut index: Vec<SessionMeta> =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    let changed = merge_discovered(&mut index, &found, &BTreeSet::new());
                    crate::store_lock::write_bytes_replace(
                        &path,
                        &serde_json::to_vec(&index).unwrap(),
                    )?;
                    Ok(changed)
                })
                .unwrap()
            })
        })
        .collect();
    let changed = handles
        .into_iter()
        .map(|handle| usize::from(handle.join().unwrap()))
        .sum::<usize>();
    assert_eq!(changed, 1);
    let index: Vec<SessionMeta> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(index.len(), 1);
}
