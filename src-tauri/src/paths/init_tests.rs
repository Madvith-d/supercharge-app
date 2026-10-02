use super::*;
use crate::paths::init_benchmark::TestHome;
use crate::paths::{ensure_app_dirs, ensure_app_dirs_initialized};
use std::sync::{Arc, Barrier};

fn context(root: PathBuf) -> Context {
    Context {
        root,
        legacy_source: None,
    }
}

fn assert_dirs(root: &std::path::Path) {
    for relative in REQUIRED_DIRS {
        assert!(root.join(relative).is_dir(), "missing {relative}");
    }
}

fn attempts(context: &Context) -> usize {
    INITIALIZER.get().unwrap().state.lock().attempts[context]
}

#[test]
fn warm_read_callsites_initialize_once_without_caching_settings() {
    let home = TestHome::new();
    let context = Context::capture().unwrap();
    for _ in 0..3 {
        assert_eq!(crate::store::load_settings().locale, "system");
        assert!(crate::store::load_sessions_index().is_empty());
        assert!(crate::store::load_automations().is_empty());
        assert!(crate::secrets::load_secrets_disk_only()
            .official_api_key
            .is_none());
        assert_eq!(
            crate::paths::resolve_agent_supercharge_home("independent"),
            home.root.join("agent-home")
        );
        assert_eq!(
            crate::paths::resolve_inference_supercharge_home("shared", true),
            home.root.join("agent-home")
        );
    }
    assert_eq!(attempts(&context), 1);
    assert_dirs(&home.root);

    let settings = crate::store::AppSettings {
        locale: "de".into(),
        ..Default::default()
    };
    fs::write(
        home.root.join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    assert_eq!(crate::store::load_settings().locale, "de");
    fs::write(
        home.root.join("secrets.json"),
        br#"{"keychainHasOfficial":true}"#,
    )
    .unwrap();
    assert!(crate::store::load_settings().store_api_keys_in_keychain);
    assert_eq!(attempts(&context), 1);
}

#[test]
fn projects_read_preserves_existing_index_migration_repair() {
    let home = TestHome::new();
    let context = Context::capture().unwrap();
    ensure_app_dirs_initialized().unwrap();
    for expected in 2..=4 {
        assert!(crate::store::load_projects().is_empty());
        // Retired General-project migration still calls update_sessions_index.
        assert_eq!(attempts(&context), expected);
    }
    assert_dirs(&home.root);
}

#[test]
fn explicit_save_repairs_and_primes_read_initialization() {
    let home = TestHome::new();
    let context = Context::capture().unwrap();
    let mut settings = crate::store::AppSettings::default();
    crate::store::save_settings(&settings).unwrap();
    assert_eq!(attempts(&context), 1);
    fs::remove_dir_all(home.root.join("logs")).unwrap();
    crate::store::load_settings();
    assert!(
        !home.root.join("logs").exists(),
        "warm read is not full repair"
    );
    settings.locale = "ja".into();
    crate::store::save_settings(&settings).unwrap();
    assert_dirs(&home.root);
    assert_eq!(crate::store::load_settings().locale, "ja");
    assert_eq!(attempts(&context), 2);
}

#[test]
fn explicit_general_workspace_recreates_its_directory() {
    let home = TestHome::new();
    ensure_app_dirs_initialized().unwrap();
    fs::remove_dir_all(home.root.join("workspaces")).unwrap();
    let dir = crate::store::ensure_general_workspace_dir().unwrap();
    assert_eq!(dir, home.root.join("workspaces/general"));
    assert!(dir.is_dir());
    assert_eq!(attempts(&Context::capture().unwrap()), 1);
}

#[test]
fn directory_failures_retry_and_failed_repair_invalidates_success() {
    let home = TestHome::new();
    let context = Context::capture().unwrap();
    fs::write(home.root.join("projects"), b"blocker").unwrap();
    assert!(ensure_app_dirs_initialized().is_err());
    fs::remove_file(home.root.join("projects")).unwrap();
    ensure_app_dirs_initialized().unwrap();
    assert_eq!(attempts(&context), 2);

    fs::remove_dir(home.root.join("logs")).unwrap();
    fs::write(home.root.join("logs"), b"blocker").unwrap();
    assert!(ensure_app_dirs().is_err());
    assert!(ensure_app_dirs_initialized().is_err());
    fs::remove_file(home.root.join("logs")).unwrap();
    ensure_app_dirs_initialized().unwrap();
    assert_eq!(attempts(&context), 5);
    assert_dirs(&home.root);
}

#[test]
fn best_effort_migration_failure_is_retried_until_success() {
    let home = TestHome::new();
    let blocker = home.root.join("legacy");
    let source = blocker.join("source");
    #[cfg(not(windows))]
    fs::write(&blocker, b"not a directory").unwrap();
    #[cfg(windows)]
    let locked_source = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::create_dir_all(&source).unwrap();
        let settings = source.join("settings.json");
        fs::write(&settings, b"{\"locale\":\"de\"}").unwrap();
        // A file parent reports NotFound on Windows; deny sharing instead.
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(settings)
            .unwrap()
    };
    let context = Context {
        root: home.root.join("destination"),
        legacy_source: Some(source.clone()),
    };
    assert!(
        crate::paths::legacy_app_data_migration::migrate_legacy_app_data(&source, &context.root,)
            .is_err()
    );
    let init = Initializer::default();
    init.ensure(context.clone(), false).unwrap();
    assert_dirs(&context.root);
    assert!(init.state.lock().completed.is_empty());
    init.ensure(context.clone(), true).unwrap();
    assert!(init.state.lock().completed.is_empty());

    #[cfg(not(windows))]
    fs::remove_file(&blocker).unwrap();
    #[cfg(windows)]
    drop(locked_source);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("settings.json"), b"{\"locale\":\"de\"}").unwrap();
    init.ensure(context.clone(), false).unwrap();
    assert_eq!(
        fs::read(context.root.join("settings.json")).unwrap(),
        b"{\"locale\":\"de\"}"
    );
    assert!(context.root.join(".legacy-grok-app-migration-v1").is_file());
    init.ensure(context.clone(), false).unwrap();
    assert_eq!(init.state.lock().attempts[&context], 3);
}

#[test]
fn migration_context_is_part_of_cache_key() {
    let home = TestHome::new();
    let init = Initializer::default();
    let excluded = context(home.root.join("destination"));
    init.ensure(excluded.clone(), false).unwrap();
    let source = home.root.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("settings.json"), b"{}").unwrap();
    let eligible = Context {
        root: excluded.root.clone(),
        legacy_source: Some(source),
    };
    init.ensure(eligible.clone(), false).unwrap();
    assert!(eligible.root.join("settings.json").is_file());
    let different_source = Context {
        root: excluded.root.clone(),
        legacy_source: Some(home.root.join("other-source")),
    };
    init.ensure(different_source, false).unwrap();
    assert_eq!(init.state.lock().completed.len(), 3);
}

#[test]
fn override_precedence_and_empty_presence_control_migration() {
    let home = TestHome::new();
    let legacy = home.root.join("legacy");
    std::env::set_var("GROK_APP_HOME", &legacy);
    assert_eq!(Context::capture().unwrap().root, home.root);
    std::env::set_var("SUPERCHARGE_APP_HOME", "");
    assert_eq!(Context::capture().unwrap().root, legacy);
    assert!(Context::capture().unwrap().legacy_source.is_none());
    std::env::remove_var("SUPERCHARGE_APP_HOME");
    assert_eq!(Context::capture().unwrap().root, legacy);
    std::env::set_var("GROK_APP_HOME", "");
    assert!(Context::capture().unwrap().legacy_source.is_none());
    std::env::remove_var("GROK_APP_HOME");
    let default = Context::capture().unwrap();
    assert!(default.legacy_source.is_some());
    std::env::set_var("SUPERCHARGE_APP_HOME", "");
    let empty_current = Context::capture().unwrap();
    assert_eq!(empty_current.root, default.root);
    assert!(empty_current.legacy_source.is_none());
    // Default roots above are inspected only, never initialized.
}

#[test]
fn captured_context_does_not_reread_environment_during_creation() {
    let home = TestHome::new();
    let a = home.root.join("a");
    let b = home.root.join("b");
    std::env::set_var("SUPERCHARGE_APP_HOME", &a);
    let captured = Context::capture().unwrap();
    std::env::set_var("SUPERCHARGE_APP_HOME", &b);
    Initializer::default().ensure(captured, false).unwrap();
    assert_dirs(&a);
    assert!(!b.exists());
}

#[test]
fn relative_override_uses_same_absolute_root_and_cache_entry() {
    let cwd = std::env::current_dir().unwrap();
    let home = TestHome::in_dir(&cwd);
    let relative = std::path::Path::new(".").join(home.root.file_name().unwrap());
    std::env::set_var("SUPERCHARGE_APP_HOME", &relative);
    let captured = Context::capture().unwrap();
    assert_eq!(crate::paths::app_data_root(), home.root);
    assert_eq!(ensure_app_dirs_initialized().unwrap(), home.root);
    std::env::set_var("SUPERCHARGE_APP_HOME", &home.root);
    ensure_app_dirs_initialized().unwrap();
    assert_eq!(Context::capture().unwrap(), captured);
    assert_eq!(attempts(&captured), 1);
}

#[test]
fn roots_a_b_keep_separate_initialization_and_disk_settings() {
    let home = TestHome::new();
    let mut contexts = Vec::new();
    for name in ["a", "b"] {
        let root = home.root.join(name);
        std::env::set_var("SUPERCHARGE_APP_HOME", &root);
        ensure_app_dirs_initialized().unwrap();
        contexts.push(Context::capture().unwrap());
        let settings = crate::store::AppSettings {
            locale: name.into(),
            ..Default::default()
        };
        fs::write(
            root.join("settings.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
    }
    for name in ["a", "b", "a", "b"] {
        std::env::set_var("SUPERCHARGE_APP_HOME", home.root.join(name));
        assert_eq!(crate::store::load_settings().locale, name);
    }
    for context in contexts {
        assert_eq!(attempts(&context), 1);
    }
}

#[test]
fn deleted_root_is_initialized_again() {
    let home = TestHome::new();
    ensure_app_dirs_initialized().unwrap();
    fs::remove_dir_all(&home.root).unwrap();
    ensure_app_dirs_initialized().unwrap();
    assert_dirs(&home.root);
    assert_eq!(attempts(&Context::capture().unwrap()), 2);
}

#[cfg(unix)]
#[test]
fn replaced_root_is_initialized_again() {
    let home = TestHome::new();
    let root = home.root.join("data");
    std::env::set_var("SUPERCHARGE_APP_HOME", &root);
    ensure_app_dirs_initialized().unwrap();
    fs::rename(&root, home.root.join("old-data")).unwrap();
    fs::create_dir(&root).unwrap();
    ensure_app_dirs_initialized().unwrap();
    assert_dirs(&root);
    assert_eq!(attempts(&Context::capture().unwrap()), 2);
}

#[test]
fn concurrent_cold_reads_complete_one_initialization() {
    let home = TestHome::new();
    let barrier = Arc::new(Barrier::new(24));
    std::thread::scope(|scope| {
        for _ in 0..24 {
            let barrier = barrier.clone();
            let root = &home.root;
            scope.spawn(move || {
                barrier.wait();
                assert_eq!(ensure_app_dirs_initialized().unwrap(), *root);
                assert_dirs(root);
                crate::store::load_settings();
            });
        }
    });
    assert_eq!(attempts(&Context::capture().unwrap()), 1);
}

#[test]
fn concurrent_explicit_repairs_share_read_initialization_lock() {
    let home = TestHome::new();
    ensure_app_dirs_initialized().unwrap();
    fs::remove_dir(home.root.join("logs")).unwrap();
    let barrier = Arc::new(Barrier::new(24));
    std::thread::scope(|scope| {
        for index in 0..24 {
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                if index % 3 == 0 {
                    ensure_app_dirs().unwrap();
                } else {
                    ensure_app_dirs_initialized().unwrap();
                }
            });
        }
    });
    assert_dirs(&home.root);
    assert_eq!(attempts(&Context::capture().unwrap()), 9);
}

#[test]
fn settings_migrations_can_reenter_initialization_without_deadlock() {
    let home = TestHome::new();
    let settings = crate::store::AppSettings {
        locale: "en".into(),
        locale_follow_system_migrated: false,
        supercharge_model_default_migrated: false,
        model_id: Some("grok-4.6".into()),
        ..Default::default()
    };
    fs::write(
        home.root.join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || send.send(crate::store::load_settings()).unwrap());
    let loaded = recv
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("settings migration must not retain init lock");
    worker.join().unwrap();
    assert_eq!(loaded.locale, "system");
    assert!(loaded.model_id.is_none());
    assert!(loaded.supercharge_model_default_migrated);
    assert_eq!(crate::store::load_settings().locale, "system");
}
