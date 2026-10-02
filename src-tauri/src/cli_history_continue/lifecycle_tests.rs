use super::*;
use std::sync::atomic::AtomicU64;

fn rediscover_fixture(f: &StoreFixture) -> Vec<SessionMeta> {
    let suppression_path = crate::paths::app_data_root().join("cli_history_suppressed.json");
    let suppressed = if suppression_path.exists() {
        serde_json::from_slice(&fs::read(suppression_path).unwrap()).unwrap()
    } else {
        Default::default()
    };
    let found = crate::cli_history::discover(&[
        fs::canonicalize(f.native.root.join("terminal")).unwrap(),
        f.native.home.clone(),
    ]);
    let mut index = crate::store::load_sessions_index();
    crate::cli_history::merge_discovered(&mut index, &found, &suppressed);
    index
}

#[test]
fn preparation_errors_remove_only_fresh_copies() {
    for failure in [
        "lock-open",
        "lock-held",
        "summary-read",
        "summary-cwd",
        "summary-write",
    ] {
        let f = Fixture::new();
        let original = snapshot(&f.source).unwrap();
        let prepared = f.fork().unwrap();
        let directory = prepared.directory.clone();
        let lock_path = directory.join("app_execution.lock");
        let summary_path = directory.join("summary.json");
        let mut held_lock = None;
        match failure {
            "lock-open" => fs::create_dir(&lock_path).unwrap(),
            "lock-held" => {
                let lock = fs::File::create(&lock_path).unwrap();
                fs2::FileExt::try_lock_exclusive(&lock).unwrap();
                held_lock = Some(lock);
            }
            "summary-read" => fs::write(&summary_path, b"invalid JSON").unwrap(),
            "summary-cwd" => {
                let mut summary: Value =
                    serde_json::from_slice(&fs::read(&summary_path).unwrap()).unwrap();
                summary["info"]["cwd"] = json!("/another-project");
                fs::write(&summary_path, serde_json::to_vec(&summary).unwrap()).unwrap();
            }
            "summary-write" => fs::create_dir(directory.join("summary.json.tmp")).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            finish_preparation(prepared, &f.cwd, "custom").is_err(),
            "{failure}"
        );
        drop(held_lock);
        assert!(!directory.exists(), "{failure}");
        assert_eq!(snapshot(&f.source).unwrap(), original, "{failure}");
    }
}

#[test]
fn preparation_errors_and_abandoned_retries_preserve_reused_state() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let prepared = f.prepare(&meta);
    record_execution(&meta, &prepared).unwrap();
    let directory = prepared.directory.clone();
    let id = prepared.agent_session_id.clone();
    let before = snapshot(&directory).unwrap();
    assert!(prepare(&meta, &f.native.home, &f.native.cwd, "custom").is_err());
    assert_eq!(snapshot(&directory).unwrap(), before);
    drop(prepared);
    assert!(prepare(&meta, &f.native.home, Path::new("/wrong-cwd"), "custom").is_err());
    assert_eq!(snapshot(&directory).unwrap(), before);
    fs::create_dir(directory.join("summary.json.tmp")).unwrap();
    assert!(prepare(&meta, &f.native.home, &f.native.cwd, "custom").is_err());
    assert_eq!(snapshot(&directory).unwrap(), before);
    fs::remove_dir(directory.join("summary.json.tmp")).unwrap();
    let retry = f.prepare(&meta);
    assert_eq!(retry.agent_session_id, id);
    drop(retry);
    assert!(directory.is_dir());
    fs::write(directory.join("summary.json"), b"invalid JSON").unwrap();
    assert!(prepare(&meta, &f.native.home, &f.native.cwd, "custom").is_err());
    assert_eq!(
        fs::read(directory.join("summary.json")).unwrap(),
        b"invalid JSON"
    );
    assert!(directory.join("chat_history.jsonl").is_file());
}

#[test]
fn superseded_commit_drops_unrecorded_copy_and_retries_from_untouched_source() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let original = snapshot(&f.native.source).unwrap();
    let prepared = f.prepare(&meta);
    let directory = prepared.directory.clone();
    let generation = AtomicU64::new(2);
    let result = commit_if_current(&generation, 1, || {
        record_execution(&meta, &prepared)?;
        Ok(prepared)
    });
    assert!(result.unwrap_err().contains("superseded"));
    assert!(!directory.exists());
    assert_eq!(snapshot(&f.native.source).unwrap(), original);
    let retry = f.prepare(&meta);
    assert_ne!(retry.directory, directory);
}

#[test]
fn superseded_retry_never_removes_recorded_execution() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let prepared = f.prepare(&meta);
    record_execution(&meta, &prepared).unwrap();
    let directory = prepared.directory.clone();
    drop(prepared);
    let retry = f.prepare(&meta);
    let before = snapshot(&directory).unwrap();
    let generation = AtomicU64::new(2);
    let result = commit_if_current(&generation, 1, || Ok(retry));
    assert!(result.is_err());
    assert_eq!(snapshot(&directory).unwrap(), before);
    assert_eq!(f.prepare(&meta).directory, directory);
}

#[test]
fn failed_execution_record_removes_fresh_copy_without_claiming_source() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let original = snapshot(&f.native.source).unwrap();
    let app_dir = crate::paths::session_dir(&meta.id);
    fs::create_dir(app_dir.join("cli_execution.json.tmp")).unwrap();
    let prepared = f.prepare(&meta);
    let directory = prepared.directory.clone();
    let generation = AtomicU64::new(1);
    let result = commit_if_current(&generation, 1, || {
        record_execution(&meta, &prepared)?;
        Ok(prepared)
    });
    assert!(result.is_err());
    assert!(!directory.exists());
    assert!(!app_dir.join("cli_execution.json").exists());
    assert!(!app_dir.join("messages.json").exists());
    assert_eq!(snapshot(&f.native.source).unwrap(), original);
    fs::remove_dir(app_dir.join("cli_execution.json.tmp")).unwrap();
    let retry = f.prepare(&meta);
    assert_ne!(retry.directory, directory);
}

#[test]
fn partial_commit_keeps_pending_fork_execution_for_retry() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let source = f.source_meta();
    let child = crate::store::fork_session(&source.id, Some(0), None, true).unwrap();
    let prepared = f.prepare(&child);
    let directory = prepared.directory.clone();
    let id = prepared.agent_session_id.clone();
    let before = snapshot(&directory).unwrap();
    let journal = crate::paths::session_dir(&child.id).join("messages.json");
    let journal_before = fs::read(&journal).unwrap();
    let generation = AtomicU64::new(1);
    let result = commit_if_current(&generation, 1, || {
        record_execution(&child, &prepared)?;
        crate::cli_history::materialize_execution(&child.id, &prepared.directory)?;
        // A failed index write leaves the parent's agent identity in metadata.
        let _prepared = prepared;
        Err::<(), _>("index write failed".to_string())
    });
    assert!(result.is_err());
    assert_eq!(snapshot(&directory).unwrap(), before);
    let saved = crate::store::load_sessions_index()
        .into_iter()
        .find(|row| row.id == child.id)
        .unwrap();
    assert_eq!(saved.agent_session_id.as_deref(), Some("source"));
    assert_eq!(saved.fork_rewind_prompt_index, Some(0));
    let retry = f.prepare(&saved);
    assert_eq!(retry.directory, directory);
    assert_eq!(retry.agent_session_id, id);
    assert_eq!(fs::read(journal).unwrap(), journal_before);
    let mut unrelated = saved;
    unrelated.agent_session_id = Some("unrelated-execution".into());
    assert!(execution_directory(&unrelated).is_none());
}

#[test]
fn failed_materialization_keeps_recorded_copy_for_retry() {
    let f = StoreFixture::new();
    let meta = f.source_meta();
    let prepared = f.prepare(&meta);
    let directory = prepared.directory.clone();
    let app_dir = crate::paths::session_dir(&meta.id);
    fs::create_dir(app_dir.join("messages.json.tmp")).unwrap();
    let generation = AtomicU64::new(1);
    let result = commit_if_current(&generation, 1, || {
        record_execution(&meta, &prepared)?;
        crate::cli_history::materialize_execution(&meta.id, &prepared.directory)?;
        Ok(prepared)
    });
    assert!(result.is_err());
    assert!(directory.is_dir());
    assert!(app_dir.join("cli_execution.json").is_file());
    fs::remove_dir(app_dir.join("messages.json.tmp")).unwrap();
    let saved = crate::store::load_sessions_index()
        .into_iter()
        .find(|row| row.id == meta.id)
        .unwrap();
    let retry = f.prepare(&saved);
    assert_eq!(retry.directory, directory);
    crate::cli_history::materialize_execution(&meta.id, &retry.directory).unwrap();
    assert!(app_dir.join("messages.json").is_file());
}

#[test]
fn failed_home_migration_commit_reuses_recorded_copy_not_previous_execution() {
    let f = StoreFixture::new();
    let mut meta = f.source_meta();
    let prepared = f.prepare(&meta);
    record_execution(&meta, &prepared).unwrap();
    meta.agent_session_id = Some(prepared.agent_session_id.clone());
    meta.cli_source.as_mut().unwrap().app_owned = true;
    crate::store::update_session_meta(&meta).unwrap();
    let previous_directory = prepared.directory.clone();
    drop(prepared);
    let previous = snapshot(&previous_directory).unwrap();
    let next_home = fs::canonicalize(f.native.root.join("terminal")).unwrap();
    let migrated = prepare(&meta, &next_home, &f.native.cwd, "custom").unwrap();
    let next_directory = migrated.directory.clone();
    record_execution(&meta, &migrated).unwrap();
    drop(migrated);
    let retry = prepare(&meta, &next_home, &f.native.cwd, "custom").unwrap();
    assert_eq!(retry.directory, next_directory);
    assert_ne!(retry.directory, previous_directory);
    assert_eq!(snapshot(&previous_directory).unwrap(), previous);
}

#[test]
fn deleting_parent_row_preserves_native_state_needed_by_pending_child() {
    let f = StoreFixture::new();
    put_user_turns(&f.native);
    let mut parent = f.source_meta();
    let prepared = f.prepare(&parent);
    record_execution(&parent, &prepared).unwrap();
    crate::cli_history::materialize_execution(&parent.id, &prepared.directory).unwrap();
    parent.agent_session_id = Some(prepared.agent_session_id.clone());
    parent.cli_source.as_mut().unwrap().app_owned = true;
    crate::store::update_session_meta(&parent).unwrap();
    let execution = prepared.directory.clone();
    drop(prepared);
    let child = crate::store::fork_session(&parent.id, Some(0), None, true).unwrap();
    let original = snapshot(&f.native.source).unwrap();
    let execution_before = snapshot(&execution).unwrap();
    let child_journal = crate::paths::session_dir(&child.id).join("messages.json");
    let journal_before = fs::read(&child_journal).unwrap();
    crate::store::delete_session(&parent.id).unwrap();
    assert!(!crate::paths::session_dir(&parent.id).exists());
    assert_eq!(snapshot(&execution).unwrap(), execution_before);
    let child_execution = f.prepare(&child);
    assert_ne!(child_execution.directory, execution);
    assert_eq!(fs::read(&child_journal).unwrap(), journal_before);
    assert_eq!(snapshot(&f.native.source).unwrap(), original);
    let discovered = rediscover_fixture(&f);
    assert!(discovered.iter().any(|row| row
        .cli_source
        .as_ref()
        .is_some_and(|source| !source.app_owned && source.agent_session_id == "source")));
    crate::store::delete_session(&child.id).unwrap();
    assert_eq!(snapshot(&execution).unwrap(), execution_before);
}

#[test]
fn deleting_source_row_keeps_native_files_and_source_suppression() {
    let f = StoreFixture::new();
    let source = f.source_meta();
    let original = snapshot(&f.native.source).unwrap();
    crate::store::delete_session(&source.id).unwrap();
    assert_eq!(snapshot(&f.native.source).unwrap(), original);
    assert!(!crate::paths::session_dir(&source.id).exists());
    assert!(rediscover_fixture(&f).iter().all(|row| row
        .cli_source
        .as_ref()
        .is_none_or(|source| source.agent_session_id != "source")));
}
