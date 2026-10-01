use super::*;
use std::ffi::OsString;

struct MetadataHome {
    path: PathBuf,
    previous: Option<OsString>,
}

impl MetadataHome {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("project-metadata-{}", Uuid::new_v4()));
        let previous = std::env::var_os("SUPERCHARGE_APP_HOME");
        std::env::set_var("SUPERCHARGE_APP_HOME", &path);
        Self { path, previous }
    }
}

impl Drop for MetadataHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("SUPERCHARGE_APP_HOME", value),
            None => std::env::remove_var("SUPERCHARGE_APP_HOME"),
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn project(id: &str, path: &Path) -> Project {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "path": path,
        "trusted": true, "lastOpenedAt": "2026-01-01T00:00:00Z", "pathOk": false
    }))
    .unwrap()
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut files: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            let bytes = fs::read(&path).unwrap();
            (path, bytes, modified)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn metadata_missing_home_does_not_initialize() {
    let _home_lock = crate::paths::APP_HOME_ENV_LOCK.lock().unwrap();
    let home = MetadataHome::new();
    assert!(load_projects_metadata().is_empty());
    assert!(!home.path.exists());
}

#[test]
fn metadata_corrupt_store_is_not_quarantined() {
    let _home_lock = crate::paths::APP_HOME_ENV_LOCK.lock().unwrap();
    let home = MetadataHome::new();
    fs::create_dir_all(&home.path).unwrap();
    for bytes in ["{broken", "", "[{}]"] {
        fs::write(projects_file(), bytes).unwrap();
        let before = snapshot(&home.path);
        assert!(load_projects_metadata().is_empty());
        assert_eq!(snapshot(&home.path), before);
    }
}

#[test]
fn metadata_preserves_health_alias_duplicates_and_session_bindings() {
    let _home_lock = crate::paths::APP_HOME_ENV_LOCK.lock().unwrap();
    let home = MetadataHome::new();
    fs::create_dir_all(&home.path).unwrap();
    let mut local = project("local", &home.path);
    local.trusted = false;
    let mut legacy = project("legacy-ssh", Path::new("/missing/remote/work"));
    legacy.name = "host:work".into();
    let mut remote = legacy.clone();
    remote.id = "explicit-ssh".into();
    remote.ssh_alias = Some("host".into());
    remote.pinned = true;
    let general = project(GENERAL_PROJECT_ID, &home.path);
    let mut system = project("old-system", &home.path);
    system.system = true;
    fs::write(
        projects_file(),
        serde_json::to_vec(&vec![local, legacy, remote, general, system]).unwrap(),
    )
    .unwrap();
    fs::write(
        sessions_index_file(),
        br#"[{"id":"bound","projectId":"system:general"},{"id":"ssh","projectId":"legacy-ssh"}]"#,
    )
    .unwrap();
    fs::write(settings_file(), b"{}").unwrap();
    let before = snapshot(&home.path);

    let rows = load_projects_metadata();
    assert_eq!(
        rows.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["explicit-ssh", "local", "legacy-ssh"]
    );
    assert!(rows.iter().all(|p| !p.path_ok));
    assert!(!rows[1].trusted);
    assert!(rows[2].ssh_alias.is_none());
    assert_eq!(snapshot(&home.path), before);
}

#[test]
fn validated_load_still_checks_local_health() {
    let _scope_lock = crate::path_scope::TEST_LOCK.blocking_lock();
    let _home_lock = crate::paths::APP_HOME_ENV_LOCK.lock().unwrap();
    let home = MetadataHome::new();
    fs::create_dir_all(&home.path).unwrap();
    let present = project("present", &home.path);
    let mut absent = project("absent", &home.path.join("missing"));
    absent.path_ok = true;
    fs::write(
        projects_file(),
        serde_json::to_vec(&vec![present, absent]).unwrap(),
    )
    .unwrap();
    let rows = load_projects();
    assert!(rows[0].path_ok);
    assert!(!rows[1].path_ok);
    assert!(crate::paths::general_workspace_dir().is_dir());
}
