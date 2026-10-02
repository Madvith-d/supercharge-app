use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::Instant;

pub(super) struct TestHome {
    pub(super) root: PathBuf,
    previous: [Option<OsString>; 2],
    _lock: MutexGuard<'static, ()>,
}

impl TestHome {
    pub(super) fn new() -> Self {
        Self::in_dir(&std::env::temp_dir())
    }

    pub(super) fn in_dir(parent: &Path) -> Self {
        let lock = super::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = parent.join(format!("supercharge-init-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create isolated root");
        let previous = [
            std::env::var_os("SUPERCHARGE_APP_HOME"),
            std::env::var_os("GROK_APP_HOME"),
        ];
        std::env::set_var("SUPERCHARGE_APP_HOME", &root);
        std::env::remove_var("GROK_APP_HOME");
        Self {
            root,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        for (name, value) in ["SUPERCHARGE_APP_HOME", "GROK_APP_HOME"]
            .into_iter()
            .zip(&self.previous)
        {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Copy this module unchanged to the released baseline and register it in paths.rs.
#[test]
#[ignore = "manual disk-backed settings read benchmark"]
fn warm_settings_reads() {
    let home = TestHome::new();
    super::ensure_app_dirs().expect("initialize benchmark root");
    let fixture = crate::store::AppSettings {
        locale: "en".into(),
        sidebar_collapse_default_migrated: true,
        ..Default::default()
    };
    let settings = serde_json::to_vec_pretty(&fixture).unwrap();
    fs::write(home.root.join("settings.json"), &settings).unwrap();
    fs::write(home.root.join("secrets.json"), b"{}").unwrap();

    let first = Instant::now();
    assert_eq!(crate::store::load_settings().locale, "en");
    let first_elapsed = first.elapsed();
    const READS: u32 = 1000;
    let start = Instant::now();
    for _ in 0..READS {
        let settings = std::hint::black_box(crate::store::load_settings());
        assert_eq!(settings.locale, "en");
        assert!(!settings.store_api_keys_in_keychain);
    }
    let elapsed = start.elapsed();
    assert_eq!(fs::read(home.root.join("settings.json")).unwrap(), settings);
    println!(
        "warm_settings_reads: first={first_elapsed:?}, reads={READS}, total={elapsed:?}, per_read={:?}",
        elapsed / READS
    );
}
