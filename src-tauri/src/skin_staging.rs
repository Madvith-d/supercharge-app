//! Upload staging (IDB → disk chunks). Inspect uses a separate tree.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde::Serialize;
use uuid::Uuid;

use crate::paths;
use crate::skin_disk;

const UPLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const INSPECT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;

static UPLOAD_LOCK: Mutex<Option<String>> = Mutex::new(None);

fn process_owned_paths() -> &'static parking_lot::Mutex<HashSet<PathBuf>> {
    static OWNED: OnceLock<parking_lot::Mutex<HashSet<PathBuf>>> = OnceLock::new();
    OWNED.get_or_init(parking_lot::Mutex::default)
}

/// Register ownership before creation; retain it through consume/save until process exit.
pub(crate) fn create_process_owned_dir(dir: &Path) -> io::Result<()> {
    let absolute = std::path::absolute(dir)?;
    let parent = absolute.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "staging directory needs a parent",
        )
    })?;
    let name = absolute.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "staging directory needs a name",
        )
    })?;
    fs::create_dir_all(parent)?;
    let canonical = parent.canonicalize()?.join(name);
    let mut owned = process_owned_paths().lock();
    owned.insert(canonical.clone());
    fs::create_dir_all(&canonical)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingBegin {
    pub upload_id: String,
}

fn gc_tree(root: &Path, ttl: Duration, process_started: SystemTime, now: SystemTime) -> usize {
    let Ok(rd) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for ent in rd.flatten() {
        if !ent.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        // Current-process staging can still be in use after its nominal TTL.
        if modified >= process_started {
            continue;
        }
        if !now.duration_since(modified).is_ok_and(|age| age >= ttl) {
            continue;
        }
        let Ok(canonical) = ent.path().canonicalize() else {
            continue;
        };
        // Serialize the ownership check and deletion with staging creation.
        let owned = process_owned_paths().lock();
        if !owned.contains(&canonical) && fs::remove_dir_all(ent.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn gc_expired_staging(root: &Path, process_started: SystemTime) -> usize {
    let staging = root.join("skin-presets").join(".staging");
    let now = SystemTime::now();
    gc_tree(&staging.join("inspect"), INSPECT_TTL, process_started, now)
        + gc_tree(&staging.join("upload"), UPLOAD_TTL, process_started, now)
}

/// Cleanup is independent of reads and never resolves a different app home mid-pass.
pub fn start_background_cleanup(root: PathBuf, process_started: SystemTime) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop {
            let root = root.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                let started = std::time::Instant::now();
                let removed = gc_expired_staging(&root, process_started);
                tracing::debug!(
                    removed,
                    elapsed_ms = started.elapsed().as_millis(),
                    "skin staging cleanup complete"
                );
            })
            .await;
            if let Err(error) = result {
                tracing::warn!(%error, "skin staging cleanup worker failed");
            }
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

pub fn upload_dir(id: &str) -> PathBuf {
    paths::skin_staging_upload_dir().join(id)
}

pub fn inspect_dir(id: &str) -> PathBuf {
    paths::skin_staging_inspect_dir().join(id)
}

pub fn begin_upload() -> Result<StagingBegin, String> {
    let mut slot = UPLOAD_LOCK
        .lock()
        .map_err(|_| "busy: upload lock poisoned".to_string())?;
    if let Some(cur) = slot.as_ref() {
        if upload_dir(cur).exists() {
            return Err("busy: an upload is already in progress".into());
        }
    }
    let id = Uuid::new_v4().to_string();
    skin_disk::preflight(64 * 1024)?;
    let dir = upload_dir(&id);
    create_process_owned_dir(&dir).map_err(|e| format!("invalid_pack: mkdir upload: {e}"))?;
    *slot = Some(id.clone());
    Ok(StagingBegin { upload_id: id })
}

pub fn append_upload(staging_id: &str, chunk_base64: &str) -> Result<u64, String> {
    let slot = UPLOAD_LOCK
        .lock()
        .map_err(|_| "busy: upload lock poisoned".to_string())?;
    if slot.as_deref() != Some(staging_id) {
        return Err("not_found: unknown upload staging id".into());
    }
    drop(slot);
    let dir = upload_dir(staging_id);
    if !dir.is_dir() {
        return Err("not_found: upload staging missing".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk_base64.trim())
        .map_err(|_| "invalid_pack: bad chunk base64".to_string())?;
    let blob = dir.join("blob.bin");
    let current = fs::metadata(&blob).map(|m| m.len()).unwrap_or(0);
    let next = current.saturating_add(bytes.len() as u64);
    if next > MAX_UPLOAD_BYTES {
        return Err("too_large: upload exceeds 200 MiB".into());
    }
    skin_disk::preflight(bytes.len() as u64)?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&blob)
        .map_err(|e| format!("invalid_pack: append: {e}"))?;
    f.write_all(&bytes)
        .map_err(|e| format!("invalid_pack: append write: {e}"))?;
    Ok(next)
}

pub fn abort_upload(staging_id: &str) -> Result<(), String> {
    let mut slot = UPLOAD_LOCK
        .lock()
        .map_err(|_| "busy: upload lock poisoned".to_string())?;
    if slot.as_deref() == Some(staging_id) {
        *slot = None;
    }
    let dir = upload_dir(staging_id);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("invalid_pack: abort upload: {e}"))?;
    }
    Ok(())
}

pub fn consume_upload(staging_id: &str) -> Result<PathBuf, String> {
    let mut slot = UPLOAD_LOCK
        .lock()
        .map_err(|_| "busy: upload lock poisoned".to_string())?;
    if slot.as_deref() != Some(staging_id) {
        return Err("not_found: unknown upload staging id".into());
    }
    *slot = None;
    let dir = upload_dir(staging_id);
    if !dir.is_dir() {
        return Err("not_found: upload staging missing".into());
    }
    Ok(dir)
}

pub fn abort_inspect(inspect_id: &str) -> Result<(), String> {
    crate::skin_pack::clear_current_inspect(inspect_id);
    let dir = inspect_dir(inspect_id);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("invalid_pack: abort inspect: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_expired_staging_but_retains_fresh_and_current_process_work() {
        let root = std::env::temp_dir().join(format!("staging-gc-{}", Uuid::new_v4()));
        for (case, expired, current_process, expected) in [
            ("expired", true, false, 1),
            ("fresh", false, false, 0),
            ("current", true, true, 0),
        ] {
            let tree = root.join(case);
            let stage = tree.join("stage");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("payload"), b"staged data").unwrap();
            let modified = fs::metadata(&stage).unwrap().modified().unwrap();
            let cutoff = modified + Duration::from_secs(u64::from(!current_process));
            let now = modified + if expired { UPLOAD_TTL } else { UPLOAD_TTL / 2 };
            assert_eq!(gc_tree(&tree, UPLOAD_TTL, cutoff, now), expected, "{case}");
            assert_eq!(stage.exists(), expected == 0, "{case}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_preserves_owned_paths_with_pre_start_timestamps_in_their_root_only() {
        let root = std::env::temp_dir().join(format!("staging-gc-owned-{}", Uuid::new_v4()));
        let owned_tree = root.join("owned");
        let old_tree = root.join("old");
        let owned = owned_tree.join("same-id");
        let old = old_tree.join("same-id");
        create_process_owned_dir(&owned).unwrap();
        fs::create_dir_all(&old).unwrap();
        fs::write(owned.join("payload"), b"keep").unwrap();
        let future = SystemTime::now() + UPLOAD_TTL * 2;
        assert!(fs::metadata(&owned).unwrap().modified().unwrap() < future);
        assert_eq!(gc_tree(&owned_tree, UPLOAD_TTL, future, future), 0);
        assert_eq!(gc_tree(&old_tree, UPLOAD_TTL, future, future), 1);
        assert_eq!(fs::read(owned.join("payload")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_creation_registers_ownership_before_attempting_mkdir() {
        let root = std::env::temp_dir().join(format!("staging-register-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let stage = root.join("stage");
        fs::write(&stage, b"blocker").unwrap();
        assert!(create_process_owned_dir(&stage).is_err());
        assert!(process_owned_paths()
            .lock()
            .contains(&stage.canonicalize().unwrap()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_racing_staging_creation_retains_owned_work() {
        let root = std::env::temp_dir().join(format!("staging-gc-race-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("expired")).unwrap();
        let future = SystemTime::now() + UPLOAD_TTL * 2;
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                for id in 0..24 {
                    let dir = root.join(id.to_string());
                    create_process_owned_dir(&dir).unwrap();
                    fs::write(dir.join("payload"), b"keep").unwrap();
                }
            });
            barrier.wait();
            for _ in 0..24 {
                gc_tree(&root, UPLOAD_TTL, future, future);
            }
        });
        assert!(!root.join("expired").exists());
        for id in 0..24 {
            assert_eq!(
                fs::read(root.join(id.to_string()).join("payload")).unwrap(),
                b"keep"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_does_not_create_missing_staging_directories() {
        let root = std::env::temp_dir().join(format!("staging-gc-missing-{}", Uuid::new_v4()));
        assert_eq!(gc_expired_staging(&root, SystemTime::now()), 0);
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_follow_directory_symlinks() {
        let root = std::env::temp_dir().join(format!("staging-gc-link-{}", Uuid::new_v4()));
        let tree = root.join("staging");
        let target = root.join("outside");
        fs::create_dir_all(&tree).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep"), b"keep").unwrap();
        std::os::unix::fs::symlink(&target, tree.join("link")).unwrap();
        let future = SystemTime::now() + UPLOAD_TTL * 2;
        assert_eq!(gc_tree(&tree, UPLOAD_TTL, future, future), 0);
        assert!(target.join("keep").exists());
        assert!(tree.join("link").is_symlink());
        fs::remove_dir_all(root).unwrap();
    }

    fn home_guard() -> (std::sync::MutexGuard<'static, ()>, std::path::PathBuf) {
        let g = crate::paths::APP_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "grok-skin-staging-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("GROK_APP_HOME", &tmp);
        let _ = crate::paths::ensure_app_dirs();
        (g, tmp)
    }

    #[test]
    fn cleanup_preserves_upload_after_consume_with_pre_start_timestamp() {
        let (_g, tmp) = home_guard();
        let upload = begin_upload().unwrap();
        append_upload(&upload.upload_id, "a2VlcA==").unwrap();
        let dir = consume_upload(&upload.upload_id).unwrap();
        let future = SystemTime::now() + UPLOAD_TTL * 2;
        assert_eq!(
            gc_tree(dir.parent().unwrap(), UPLOAD_TTL, future, future),
            0
        );
        assert_eq!(fs::read(dir.join("blob.bin")).unwrap(), b"keep");
        abort_upload(&upload.upload_id).unwrap();
        fs::remove_dir_all(tmp).unwrap();
        std::env::remove_var("GROK_APP_HOME");
    }

    #[test]
    fn cleanup_preserves_both_inspect_creation_paths_with_pre_start_timestamps() {
        let (_g, tmp) = home_guard();
        let manifest = br#"{"schemaVersion":1,"name":"x","skin":"default","wallpaper":null}"#;
        let library = tmp.join("library");
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("manifest.json"), manifest).unwrap();
        let archive = tmp.join("test.superchargeskin");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(manifest).unwrap();
        zip.finish().unwrap();
        for unpacked in [true, false] {
            let preview = if unpacked {
                crate::skin_pack::inspect_unpacked_dir(&library, "preset", false).unwrap()
            } else {
                crate::skin_pack::inspect_pack(&archive, "file").unwrap()
            };
            let dir = inspect_dir(&preview.id);
            let future = SystemTime::now() + INSPECT_TTL * 2;
            assert_eq!(
                gc_tree(dir.parent().unwrap(), INSPECT_TTL, future, future),
                0
            );
            assert!(dir
                .join(if unpacked {
                    ".library-ref"
                } else {
                    "manifest.json"
                })
                .is_file());
            abort_inspect(&preview.id).unwrap();
        }
        fs::remove_dir_all(tmp).unwrap();
        std::env::remove_var("GROK_APP_HOME");
    }

    #[test]
    fn materialized_upload_pack_registers_process_ownership() {
        let (_g, tmp) = home_guard();
        crate::skin_presets::save_from_upload(
            "",
            serde_json::json!({"schemaVersion": 1, "name": "x", "skin": "default", "wallpaper": null}),
        ).unwrap();
        let tree = paths::skin_staging_upload_dir().canonicalize().unwrap();
        assert!(process_owned_paths().lock().iter().any(|path| {
            path.parent() == Some(tree.as_path())
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("pack-")
        }));
        fs::remove_dir_all(tmp).unwrap();
        std::env::remove_var("GROK_APP_HOME");
    }

    #[test]
    fn second_begin_is_busy() {
        let (_g, tmp) = home_guard();
        let a = begin_upload().expect("first begin");
        let err = begin_upload().unwrap_err();
        assert!(err.starts_with("busy"), "{err}");
        abort_upload(&a.upload_id).unwrap();
        let _ = fs::remove_dir_all(&tmp);
        std::env::remove_var("GROK_APP_HOME");
    }

    #[test]
    fn inspect_does_not_block_upload() {
        let (_g, tmp) = home_guard();
        let inspect_id = Uuid::new_v4().to_string();
        fs::create_dir_all(inspect_dir(&inspect_id)).unwrap();
        let a = begin_upload().expect("upload while inspect exists");
        abort_upload(&a.upload_id).unwrap();
        let _ = fs::remove_dir_all(&tmp);
        std::env::remove_var("GROK_APP_HOME");
    }
}
