use super::*;
use std::fs;

struct RestoreHome(Option<std::ffi::OsString>);
impl Drop for RestoreHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("SUPERCHARGE_APP_HOME", v),
            None => std::env::remove_var("SUPERCHARGE_APP_HOME"),
        }
    }
}

fn with_isolated_roots(project: &Path, app: &Path, include_temp: bool, f: impl FnOnce()) {
    let _g = TEST_LOCK.blocking_lock();
    let _home = crate::paths::APP_HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("SUPERCHARGE_APP_HOME");
    std::env::set_var("SUPERCHARGE_APP_HOME", app);
    let _restore = RestoreHome(prev);
    let _ = fs::create_dir_all(app);
    let _ = fs::create_dir_all(project);
    let projects_file = app.join("projects.json");
    let _ = fs::write(&projects_file, "[]");
    // Install a deterministic root set (optional temp) so tests do not depend on the
    // real machine project list or always-on temp allow.
    let mut next = Vec::new();
    if let Ok(c) = project.canonicalize() {
        next.push(c);
    }
    if let Ok(c) = app.canonicalize() {
        next.push(c);
    } else {
        next.push(app.to_path_buf());
    }
    if include_temp {
        if let Ok(c) = std::env::temp_dir().canonicalize() {
            next.push(c);
        }
    }
    roots().write().paths = next;
    *extra_grants().write() = Vec::new();
    f();
    *extra_grants().write() = Vec::new();
    roots().write().paths.clear();
}

#[test]
fn allows_path_under_project() {
    let tmp = std::env::temp_dir().join(format!("grok-scope-{}", std::process::id()));
    let project = tmp.join("proj");
    let app = tmp.join("app");
    let _ = fs::create_dir_all(&project);
    let file = project.join("readme.md");
    fs::write(&file, "hi").unwrap();
    with_isolated_roots(&project, &app, false, || {
        assert!(is_allowed(&file));
        assert!(require_allowed(&file).is_ok());
    });
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn denies_path_outside_roots() {
    let tmp = std::env::temp_dir().join(format!("grok-scope-out-{}", std::process::id()));
    let project = tmp.join("proj");
    let app = tmp.join("app");
    let _ = fs::create_dir_all(&project);
    let _ = fs::create_dir_all(tmp.join("other"));
    let outside = tmp.join("other").join("secret.txt");
    fs::write(&outside, "secret").unwrap();
    // No global temp root — sibling of project must be denied.
    with_isolated_roots(&project, &app, false, || {
        assert!(!is_allowed(&outside));
        assert!(require_allowed(&outside).is_err());
    });
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn grant_path_allows_one_off() {
    let tmp = std::env::temp_dir().join(format!("grok-scope-grant-{}", std::process::id()));
    let project = tmp.join("proj");
    let app = tmp.join("app");
    let other = tmp.join("picked");
    let _ = fs::create_dir_all(&project);
    let _ = fs::create_dir_all(&other);
    let file = other.join("picked.md");
    fs::write(&file, "x").unwrap();
    with_isolated_roots(&project, &app, false, || {
        assert!(!is_allowed(&file));
        grant_path(&file);
        assert!(is_allowed(&file));
        let sibling = other.join("secret.key");
        fs::write(&sibling, "no").unwrap();
        assert!(
            !is_allowed(&sibling),
            "granting a file must not unlock siblings in the parent dir"
        );
    });
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn prefix_does_not_match_sibling_name() {
    // /foo should not allow /foobar
    let foo = PathBuf::from("/foo");
    let foobar = PathBuf::from("/foobar/x");
    assert!(!path_under_root(&foobar, &foo));
    assert!(path_under_root(Path::new("/foo/bar"), &foo));
}

fn project(name: &str, path: &Path) -> crate::store::Project {
    serde_json::from_value(serde_json::json!({
        "id": name, "name": name, "path": path,
        "trusted": true, "lastOpenedAt": "2026-01-01T00:00:00Z", "pathOk": false
    }))
    .unwrap()
}

#[test]
fn filters_untrusted_remote_and_retired_before_probe() {
    let local = project("local", Path::new("/scope/local"));
    let mut excluded = Vec::new();
    let mut untrusted = project("untrusted", Path::new("/scope/untrusted"));
    untrusted.trusted = false;
    excluded.push(untrusted);
    for alias in ["host", "-invalid", "bad alias", " "] {
        let mut remote = project("remote", Path::new("/scope/remote"));
        remote.ssh_alias = Some(alias.into());
        excluded.push(remote);
    }
    excluded.push(project("host:legacy", Path::new("/scope/legacy")));
    let mut empty_alias = project("host:legacy", Path::new("/scope/legacy"));
    empty_alias.ssh_alias = Some(String::new());
    excluded.push(empty_alias);
    excluded.push(project(
        crate::store::GENERAL_PROJECT_ID,
        Path::new("/scope/general"),
    ));
    let mut system = project("system", Path::new("/scope/system"));
    system.system = true;
    excluded.push(system);
    assert!(trusted_project_roots(excluded, |_| panic!("excluded project was probed")).is_empty());

    let mut blank_alias = project("blank", Path::new("/scope/blank"));
    blank_alias.ssh_alias = Some(String::new());
    let missing = project("missing", Path::new("/scope/missing"));
    let mut probes = Vec::new();
    let selected = trusted_project_roots(vec![local, blank_alias, missing], |path| {
        probes.push(path.to_path_buf());
        (path != Path::new("/scope/missing")).then(|| path.to_path_buf())
    });
    assert_eq!(
        probes,
        ["/scope/local", "/scope/blank", "/scope/missing"].map(PathBuf::from)
    );
    assert_eq!(
        selected,
        ["/scope/local", "/scope/blank"].map(PathBuf::from)
    );
}

#[test]
fn ssh_metadata_cannot_grant_an_existing_local_root() {
    let tmp = std::env::temp_dir().join(format!("scope-remote-{}", uuid::Uuid::new_v4()));
    let remote_path = tmp.join("remote");
    fs::create_dir_all(&remote_path).unwrap();
    let secret = remote_path.join("secret");
    fs::write(&secret, "private").unwrap();
    with_isolated_roots(&tmp.join("local"), &tmp.join("app"), false, || {
        let mut rows = vec![project("host:remote", &remote_path)];
        for alias in ["host", "-invalid", "bad alias", " "] {
            let mut row = project("remote", &remote_path);
            row.ssh_alias = Some(alias.into());
            rows.push(row);
        }
        let selected = trusted_project_roots(rows, |path| path.canonicalize().ok());
        assert!(selected.is_empty());
        roots().write().paths.extend(selected);
        assert!(!is_allowed(&secret));
        assert!(require_allowed(&secret).is_err());
    });
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn older_refresh_cannot_replace_newer_roots() {
    let scope = RwLock::new(ScopeRoots::default());
    let mutations = Mutex::new(());
    let (captured_tx, captured_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    std::thread::scope(|threads| {
        let scope = &scope;
        let mutations = &mutations;
        let old = threads.spawn(move || {
            refresh_roots(scope, mutations, || {
                captured_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                vec![PathBuf::from("/old")]
            });
        });
        captured_rx.recv().unwrap();
        refresh_roots(scope, mutations, || vec![PathBuf::from("/new")]);
        resume_tx.send(()).unwrap();
        old.join().unwrap();
    });
    assert_eq!(scope.read().paths, [PathBuf::from("/new")]);
}

#[test]
fn store_mutation_invalidates_snapshot_before_refresh_handoff() {
    for publish_newer in [false, true] {
        let scope = RwLock::new(ScopeRoots::default());
        let mutations = Mutex::new(());
        let stored = RwLock::new(vec![PathBuf::from("/revoked")]);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let (captured_tx, captured_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        std::thread::scope(|threads| {
            let scope = &scope;
            let mutations = &mutations;
            let stored = &stored;
            let calls = &calls;
            let old = threads.spawn(move || {
                refresh_roots(scope, mutations, || {
                    let snapshot = stored.read().clone();
                    if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        captured_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }
                    snapshot
                });
            });
            captured_rx.recv().unwrap();
            mutate_project_store(scope, mutations, || {
                *stored.write() = vec![PathBuf::from("/current")];
                assert!(scope.try_read().is_some());
                assert!(mutations.try_lock().is_none());
            });
            if publish_newer {
                refresh_roots(scope, mutations, || stored.read().clone());
            }
            resume_tx.send(()).unwrap();
            old.join().unwrap();
        });
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(scope.read().paths, [PathBuf::from("/current")]);
    }
}

#[test]
fn project_save_revokes_pending_scope_publication_synchronously() {
    let tmp = std::env::temp_dir().join(format!("scope-save-{}", uuid::Uuid::new_v4()));
    let local = tmp.join("local");
    with_isolated_roots(&local, &tmp.join("app"), false, || {
        let row = project("local", &local);
        crate::store::save_projects(&[row]).unwrap();
        let canonical = local.canonicalize().unwrap();
        assert!(roots().read().paths.contains(&canonical));
        let pending = roots().write().begin_refresh();
        crate::store::save_projects(&[]).unwrap();
        assert!(!roots().read().paths.contains(&canonical));
        assert!(!roots().write().publish(pending, vec![canonical.clone()]));
        assert!(!roots().read().paths.contains(&canonical));
    });
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn valid_project_reads_do_not_invalidate_scope_refreshes() {
    let tmp = std::env::temp_dir().join(format!("scope-read-{}", uuid::Uuid::new_v4()));
    with_isolated_roots(&tmp.join("local"), &tmp.join("app"), false, || {
        let generation = roots().read().store_generation;
        assert!(crate::store::load_projects().is_empty());
        assert!(crate::store::load_projects().is_empty());
        assert_eq!(roots().read().store_generation, generation);
    });
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn project_quarantine_invalidates_pending_scope_refresh() {
    let tmp = std::env::temp_dir().join(format!("scope-quarantine-{}", uuid::Uuid::new_v4()));
    with_isolated_roots(&tmp.join("local"), &tmp.join("app"), false, || {
        let pending = roots().write().begin_refresh();
        fs::write(crate::paths::projects_file(), "{invalid").unwrap();
        assert!(crate::store::load_projects().is_empty());
        assert!(!roots().write().publish(pending, vec![tmp.join("revoked")]));
    });
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn scope_collection_does_not_initialize_or_migrate_stores() {
    let _scope_lock = TEST_LOCK.blocking_lock();
    let _home_lock = crate::paths::APP_HOME_ENV_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!("scope-metadata-{}", uuid::Uuid::new_v4()));
    let previous = std::env::var_os("SUPERCHARGE_APP_HOME");
    struct Restore(Option<std::ffi::OsString>, PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("SUPERCHARGE_APP_HOME", value),
                None => std::env::remove_var("SUPERCHARGE_APP_HOME"),
            }
            let _ = fs::remove_dir_all(&self.1);
        }
    }
    let _restore = Restore(previous, home.clone());
    std::env::set_var("SUPERCHARGE_APP_HOME", &home);
    assert!(!collect_roots().is_empty());
    assert!(!home.exists());
    fs::create_dir_all(&home).unwrap();
    let settings = crate::store::AppSettings {
        session_data_mode: "independent".into(),
        supercharge_model_default_migrated: false,
        ..Default::default()
    };
    let fixtures = [
        ("settings.json", serde_json::to_vec(&settings).unwrap()),
        (
            "projects.json",
            serde_json::to_vec(&vec![project(crate::store::GENERAL_PROJECT_ID, &home)]).unwrap(),
        ),
        (
            "sessions_index.json",
            br#"[{"id":"session","projectId":"system:general"}]"#.to_vec(),
        ),
    ];
    for (name, bytes) in &fixtures {
        fs::write(home.join(name), bytes).unwrap();
    }
    let before: Vec<_> = fixtures
        .iter()
        .map(|(name, _)| fs::metadata(home.join(name)).unwrap().modified().unwrap())
        .collect();
    let selected = collect_roots();
    assert!(selected.contains(&crate::paths::agent_home_dir()));
    assert_eq!(fs::read_dir(&home).unwrap().count(), fixtures.len());
    for ((name, bytes), modified) in fixtures.iter().zip(before) {
        assert_eq!(fs::read(home.join(name)).unwrap(), *bytes);
        assert_eq!(
            fs::metadata(home.join(name)).unwrap().modified().unwrap(),
            modified
        );
    }
}
