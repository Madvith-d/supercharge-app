//! Metadata-only discovery of local CLI histories, independent of inference routing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{self, ChatMessageStored, SessionMeta};

pub(crate) mod legacy;
#[cfg(test)]
mod tests;
mod transcript;

const SUMMARY_LIMIT: u64 = 256 * 1024;
const SOURCE_FILES: [&str; 3] = ["summary.json", "updates.jsonl", "chat_history.jsonl"];
static SYNC_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionSource {
    pub source_home: String,
    /// Exactly `sessions/<encoded-cwd>/<session-directory>` relative to source_home.
    pub relative_dir: String,
    pub agent_session_id: String,
    pub cwd: Option<String>,
    /// Last observed source title; lets an App rename remain authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Filesystem metadata only; never a digest of the full transcript.
    #[serde(default)]
    pub revision: String,
    /// Once claimed by App execution, the App journal is authoritative.
    #[serde(default)]
    pub app_owned: bool,
}

impl CliSessionSource {
    fn key(&self) -> (String, String) {
        (self.source_home.clone(), self.relative_dir.clone())
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub source: CliSessionSource,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Roots are explicit so fixture tests never mutate process-wide environment.
pub fn source_homes(
    default_home: PathBuf,
    inherited: Option<PathBuf>,
    app_home: PathBuf,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    std::iter::once(default_home)
        .chain(inherited)
        .chain(std::iter::once(app_home))
        .filter_map(|root| fs::canonicalize(root).ok())
        .filter(|root| root.is_dir() && seen.insert(root.clone()))
        .collect()
}

pub fn allowed_source_homes() -> Vec<PathBuf> {
    source_homes(
        crate::process_util::user_home().join(".supercharge"),
        std::env::var_os("SUPERCHARGE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        crate::paths::agent_home_dir(),
    )
}

fn checked_child(
    parent: &Path,
    name: &std::ffi::OsStr,
    directory: bool,
) -> Result<PathBuf, String> {
    let path = parent.join(name);
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err("CLI history contains a symlink or unexpected file type".into());
    }
    let canonical = fs::canonicalize(&path).map_err(|e| e.to_string())?;
    if canonical.parent() != Some(parent) {
        return Err("CLI history path escapes its recorded parent".into());
    }
    Ok(canonical)
}

/// Resolve only recorded, depth-bounded provenance under an allowed canonical root.
pub fn resolve_source_in(source: &CliSessionSource, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let home = fs::canonicalize(&source.source_home).map_err(|e| e.to_string())?;
    if !roots.iter().any(|root| root == &home) || Path::new(&source.source_home) != home {
        return Err("CLI history source is not an allowed canonical home".into());
    }
    let relative = Path::new(&source.relative_dir);
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || parts[0].as_os_str() != "sessions"
        || parts[2].as_os_str() != std::ffi::OsStr::new(&source.agent_session_id)
    {
        return Err("Invalid CLI history relative directory".into());
    }
    let mut dir = home.clone();
    for part in parts {
        dir = checked_child(&dir, part.as_os_str(), true)?;
    }
    if dir.strip_prefix(&home).map(Path::as_os_str).ok() != Some(relative.as_os_str()) {
        return Err("CLI history relative directory is not canonical".into());
    }
    // The parser reads these files directly. Reject escaping leaf symlinks too.
    for file in SOURCE_FILES {
        match fs::symlink_metadata(dir.join(file)) {
            Ok(_) => {
                checked_child(&dir, std::ffi::OsStr::new(file), false)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(dir)
}

pub fn resolve_source(source: &CliSessionSource) -> Result<PathBuf, String> {
    resolve_source_in(source, &allowed_source_homes())
}

fn file_revision(dir: &Path) -> String {
    SOURCE_FILES
        .iter()
        .map(|file| match fs::metadata(dir.join(file)) {
            Ok(meta) => {
                #[cfg(unix)]
                let identity = {
                    use std::os::unix::fs::MetadataExt;
                    format!(
                        "{}:{}:{}:{}",
                        meta.dev(),
                        meta.ino(),
                        meta.ctime(),
                        meta.ctime_nsec()
                    )
                };
                #[cfg(not(unix))]
                let identity = String::new();
                format!(
                    "{file}:{}:{:?}:{:?}:{identity}",
                    meta.len(),
                    meta.modified().ok(),
                    meta.created().ok()
                )
            }
            Err(_) => format!("{file}:-"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn read_summary(dir: &Path) -> serde_json::Value {
    let Ok(file) = fs::File::open(dir.join("summary.json")) else {
        return serde_json::Value::Null;
    };
    let mut bytes = Vec::new();
    if file
        .take(SUMMARY_LIMIT + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > SUMMARY_LIMIT
    {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn text_at(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn date_at(value: &serde_json::Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    keys.iter().find_map(|key| {
        DateTime::parse_from_rfc3339(value.get(key)?.as_str()?)
            .ok()
            .map(|date| date.with_timezone(&Utc))
    })
}

fn visible_summary(summary: &serde_json::Value) -> bool {
    if !summary.is_object() {
        return false;
    }
    let hidden = summary
        .get("hidden")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| {
            summary
                .get("session_kind")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind.starts_with("subagent"))
        });
    if hidden {
        return false;
    }
    let titled = ["generated_title", "title", "session_summary"]
        .iter()
        .any(|key| text_at(summary, key).is_some());
    // Native chat rows include the system prompt and initial user-info message.
    let has_messages = summary
        .get("num_messages")
        .or_else(|| summary.get("num_chat_messages"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|count| count > 0);
    let forked = ["parent_session_id", "forked_from_session_id", "forked_at"]
        .iter()
        .any(|key| text_at(summary, key).is_some());
    titled || has_messages || forked
}

/// Scan precisely two directory levels; no history reads, row cap, or project writes.
pub fn discover(roots: &[PathBuf]) -> Vec<DiscoveredSession> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for home in roots {
        let Ok(sessions) = checked_child(home, std::ffi::OsStr::new("sessions"), true) else {
            continue;
        };
        let Ok(cwds) = fs::read_dir(&sessions) else {
            continue;
        };
        for cwd in cwds.flatten() {
            let Ok(cwd_dir) = checked_child(&sessions, &cwd.file_name(), true) else {
                continue;
            };
            let Ok(entries) = fs::read_dir(&cwd_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(agent_id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if agent_id.is_empty() || agent_id.starts_with('.') {
                    continue;
                }
                let Ok(dir) = checked_child(&cwd_dir, &entry.file_name(), true) else {
                    continue;
                };
                // App execution copies are represented by their existing App row.
                if dir.join("app_cli_origin.json").exists() {
                    continue;
                }
                let Some(home_text) = home.to_str() else {
                    continue;
                };
                let Ok(relative) = dir.strip_prefix(home) else {
                    continue;
                };
                let Some(relative) = relative.to_str() else {
                    continue;
                };
                let mut source = CliSessionSource {
                    source_home: home_text.into(),
                    relative_dir: relative.into(),
                    agent_session_id: agent_id.clone(),
                    cwd: None,
                    title: None,
                    updated_at: None,
                    revision: String::new(),
                    app_owned: false,
                };
                if resolve_source_in(&source, roots).is_err()
                    || !SOURCE_FILES.iter().any(|file| dir.join(file).is_file())
                {
                    continue;
                }
                if !seen.insert(source.key()) {
                    continue;
                }
                let summary = read_summary(&dir);
                if !visible_summary(&summary) {
                    continue;
                }
                source.cwd = summary
                    .get("info")
                    .and_then(|info| text_at(info, "cwd"))
                    .or_else(|| text_at(&summary, "cwd"))
                    .or_else(|| {
                        let name = cwd.file_name();
                        let decoded = percent_encoding::percent_decode_str(name.to_str()?)
                            .decode_utf8()
                            .ok()?
                            .into_owned();
                        Path::new(&decoded).is_absolute().then_some(decoded)
                    });
                source.revision = file_revision(&dir);
                let fallback = fs::metadata(&dir)
                    .and_then(|m| m.modified())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_default();
                let created_at =
                    date_at(&summary, &["created_at", "createdAt"]).unwrap_or(fallback);
                let updated_at = date_at(&summary, &["last_active_at", "updated_at", "updatedAt"])
                    .unwrap_or_else(|| {
                        SOURCE_FILES
                            .iter()
                            .filter_map(|file| fs::metadata(dir.join(file)).ok()?.modified().ok())
                            .max()
                            .map(DateTime::<Utc>::from)
                            .unwrap_or(created_at)
                    });
                let title = text_at(&summary, "generated_title")
                    .or_else(|| text_at(&summary, "title"))
                    .or_else(|| text_at(&summary, "session_summary"))
                    .unwrap_or(agent_id);
                source.title = Some(title.clone());
                source.updated_at = Some(updated_at);
                out.push(DiscoveredSession {
                    source,
                    title,
                    created_at,
                    updated_at,
                });
            }
        }
    }
    out.sort_by_cached_key(|entry| entry.source.key());
    out
}

fn new_meta(row: &DiscoveredSession) -> SessionMeta {
    SessionMeta {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: None,
        title: row.title.clone(),
        agent_session_id: None,
        cli_source: Some(row.source.clone()),
        created_at: row.created_at,
        updated_at: row.updated_at,
        model_id: None,
        archived: false,
        pinned: false,
        effort: None,
        mode: None,
        permission_policy: None,
        json_schema: None,
        scheduled: false,
        worktree_path: None,
        worktree_branch: None,
        is_worktree_session: false,
        plugin_dirs: Vec::new(),
        extra_rules: None,
        max_agent_turns: None,
        system_prompt_override: None,
        fork_agent_session: false,
        fork_rewind_prompt_index: None,
        no_ask_user: None,
        workspace_id: None,
        workspace_root_snapshot: None,
        workspace_capability: None,
    }
}

pub fn is_continuation_copy(meta: &SessionMeta) -> bool {
    meta.cli_source.as_ref().is_some_and(|source| {
        source.app_owned
            && meta
                .agent_session_id
                .as_deref()
                .is_some_and(|id| id != source.agent_session_id)
    })
}

type Suppressions = BTreeSet<(String, String)>;

/// Pure index mutation; callers must commit it under the existing index lock.
pub fn merge_discovered(
    list: &mut Vec<SessionMeta>,
    found: &[DiscoveredSession],
    suppressed: &BTreeSet<(String, String)>,
) -> bool {
    let mut changed = false;
    let mut source_counts = HashMap::<&str, usize>::new();
    for row in found {
        *source_counts
            .entry(&row.source.agent_session_id)
            .or_default() += 1;
    }
    let mut legacy_counts = HashMap::<String, usize>::new();
    let mut legacy_positions = HashMap::new();
    let mut source_positions = HashMap::new();
    for (index, row) in list.iter().enumerate() {
        if let Some(id) = &row.agent_session_id {
            *legacy_counts.entry(id.clone()).or_default() += 1;
            legacy_positions.insert(id.clone(), index);
        }
        if let Some(source) = &row.cli_source {
            if !is_continuation_copy(row) && !row.fork_agent_session {
                source_positions.entry(source.key()).or_insert(index);
            }
        }
    }
    for row in found {
        let key = row.source.key();
        if suppressed.contains(&key) {
            continue;
        }
        if let Some(&index) = source_positions.get(&key) {
            let existing = &mut list[index];
            let source = existing.cli_source.as_mut().expect("matched source");
            if source.revision != row.source.revision || source.cwd != row.source.cwd {
                if !source.app_owned {
                    if source.title.as_deref() == Some(existing.title.as_str()) {
                        existing.title.clone_from(&row.title);
                    }
                    existing.created_at = row.created_at;
                    existing.updated_at = if source.updated_at == Some(existing.updated_at) {
                        row.updated_at
                    } else {
                        existing.updated_at.max(row.updated_at)
                    };
                }
                source.updated_at = row.source.updated_at;
                source.title.clone_from(&row.source.title);
                source.revision.clone_from(&row.source.revision);
                source.cwd.clone_from(&row.source.cwd);
                changed = true;
            }
            continue;
        }
        let id = row.source.agent_session_id.as_str();
        let legacy = if source_counts.get(id) == Some(&1) && legacy_counts.get(id) == Some(&1) {
            legacy_positions.get(id).copied().filter(|&index| {
                let m = &list[index];
                m.cli_source.is_none()
                    && !m.fork_agent_session
                    && m.fork_rewind_prompt_index.is_none()
                    && !m.title.to_ascii_lowercase().starts_with("fork of ")
            })
        } else {
            None
        };
        if let Some(index) = legacy {
            let mut source = row.source.clone();
            source.app_owned = true;
            list[index].cli_source = Some(source);
            source_positions.insert(key, index);
        } else {
            source_positions.insert(key, list.len());
            list.push(new_meta(row));
        }
        changed = true;
    }
    changed
}

fn suppression_path() -> PathBuf {
    crate::paths::app_data_root().join("cli_history_suppressed.json")
}

fn load_suppressions(path: &Path) -> Result<Suppressions, String> {
    match fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|e| format!("Invalid CLI history suppressions: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Called only inside the sessions-index transaction; never nest store locks.
pub(crate) fn suppress_source_locked(source: &CliSessionSource) -> Result<(), String> {
    suppress_source_at(&suppression_path(), source)
}

fn suppress_source_at(path: &Path, source: &CliSessionSource) -> Result<(), String> {
    let mut entries = load_suppressions(path)?;
    if entries.insert(source.key()) {
        let raw = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
        crate::store_lock::write_bytes_replace(path, &raw)?;
    }
    Ok(())
}

fn sync_blocking() -> Result<bool, String> {
    let found = discover(&allowed_source_homes());
    store::update_sessions_index(|list| {
        let suppressed = load_suppressions(&suppression_path())?;
        Ok(merge_discovered(list, &found, &suppressed))
    })
}

struct SyncGuard;
impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNC_RUNNING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Concurrent refresh triggers coalesce into the currently running scan.
#[tauri::command]
pub async fn cli_history_sync(app: tauri::AppHandle) -> Result<bool, String> {
    use std::sync::atomic::Ordering;
    if SYNC_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(false);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = SyncGuard;
        let changed = sync_blocking()?;
        if changed {
            use tauri::Emitter;
            let _ = app.emit(
                "sessions://changed",
                serde_json::json!({"reason": "cli_history_sync"}),
            );
        }
        Ok(changed)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub use transcript::{
    materialize_execution, materialize_for_app, read_messages, read_messages_with_meta,
};
