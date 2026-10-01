//! Non-destructive native-state forks for discovered CLI sessions.
//!
//! CLI 1.3.24 locks individual writes, not session ownership. Never load an
//! external directory in place, even in the same home. Only persisted state is
//! copied; the terminal's in-memory/unflushed work remains with the terminal.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cli_history::CliSessionSource;
use crate::store::SessionMeta;

pub mod prompt_mapping;

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 8192;
const ORIGIN_FILE: &str = "app_cli_origin.json";
// Mirrors CLI storage/jsonl/copy.rs; archives and media remain self-contained.
const FILES: &[&str] = &[
    "summary.json",
    "chat_history.jsonl",
    "updates.jsonl",
    "plan.json",
    "plan_mode.json",
    "plan.md",
    "signals.json",
    "usage.json",
    "tool_state.json",
    "announcement_state.json",
    "system_prompt.txt",
    "title_refresh_idx",
];
const DIRECTORIES: &[&str] = &[
    "compaction",
    "compaction_checkpoints",
    "images",
    "videos",
    "audio",
    "files",
];

#[derive(Debug)]
pub struct PreparedContinuation {
    pub agent_session_id: String,
    pub directory: PathBuf,
    pub source_digest: String,
    pub lease: Option<std::fs::File>,
}

/// Resolve a read-only history root without confusing provenance with execution.
/// An owned session must never fall back to its diverged terminal origin.
pub fn history_directory(meta: &SessionMeta) -> Option<PathBuf> {
    let source = meta.cli_source.as_ref()?;
    if !source.app_owned
        || meta.agent_session_id.as_deref() == Some(source.agent_session_id.as_str())
    {
        return crate::cli_history::resolve_source_in(
            source,
            &crate::cli_history::allowed_source_homes(),
        )
        .ok();
    }
    if let Some(directory) = execution_directory(meta) {
        return Some(directory);
    }
    let id = meta.agent_session_id.as_deref()?;
    if id.is_empty() || Path::new(id).components().count() != 1 || id.starts_with('.') {
        return None;
    }
    let mut found = None;
    for home in crate::cli_history::allowed_source_homes() {
        for entry in fs::read_dir(home.join("sessions"))
            .ok()
            .into_iter()
            .flatten()
            .flatten()
        {
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let candidate = entry.path().join(id);
            if fs::symlink_metadata(&candidate)
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            {
                if found.is_some() {
                    return None;
                }
                found = Some(candidate);
            }
        }
    }
    found
}

pub fn fork_source(meta: &SessionMeta) -> Result<Option<CliSessionSource>, String> {
    let Some(mut source) = meta.cli_source.clone() else {
        return Ok(None);
    };
    let directory = history_directory(meta).ok_or("CLI fork source is missing or ambiguous")?;
    let home = directory
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid CLI fork source")?;
    source.source_home = home.to_string_lossy().into_owned();
    source.relative_dir = directory
        .strip_prefix(home)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    source.agent_session_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid CLI fork identity")?
        .to_owned();
    source.app_owned = true;
    Ok(Some(source))
}

fn execution_directory(meta: &SessionMeta) -> Option<PathBuf> {
    let bytes = fs::read(crate::paths::session_dir(&meta.id).join("cli_execution.json")).ok()?;
    let execution: CliSessionSource = serde_json::from_slice(&bytes).ok()?;
    if meta
        .agent_session_id
        .as_deref()
        .is_some_and(|id| id != execution.agent_session_id)
    {
        return None;
    }
    crate::cli_history::resolve_source_in(&execution, &crate::cli_history::allowed_source_homes())
        .ok()
}

pub fn lock_preparation(app_id: &str) -> Result<std::fs::File, String> {
    let directory = crate::paths::session_dir(app_id);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let path = directory.join("cli_prepare.lock");
    if fs::symlink_metadata(&path)
        .is_ok_and(|meta| meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err("Unsafe CLI preparation lock".into());
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let lock = options.open(path).map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "CLI continuation is still being prepared; retry shortly".to_string())?;
    Ok(lock)
}

/// Native copies retain their pending cut until the native rewind succeeds.
pub fn clear_failed_legacy_fork(meta: &SessionMeta) -> Result<(), String> {
    if meta.fork_agent_session && meta.cli_source.is_none() {
        crate::store::clear_session_fork_after_connect_failure(&meta.id)?;
    }
    Ok(())
}

pub fn commit_if_current<T>(
    generation: &std::sync::atomic::AtomicU64,
    expected: u64,
    commit: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if generation.load(std::sync::atomic::Ordering::SeqCst) != expected {
        return Err("CLI continuation was superseded or timed out".into());
    }
    commit()
}

/// Run on a blocking worker after route preparation, only for deliberate connects.
/// Reuse App-owned state in the same runtime home; never reload the terminal origin.
pub fn prepare(
    meta: &SessionMeta,
    runtime_home: &Path,
    cwd: &Path,
    model_id: &str,
) -> Result<PreparedContinuation, String> {
    let source = meta
        .cli_source
        .as_ref()
        .ok_or("Missing CLI history origin")?;
    let pending_execution = execution_directory(meta);
    let is_execution =
        pending_execution.is_some() || (source.app_owned && !meta.fork_agent_session);
    let directory = pending_execution
        .or_else(|| history_directory(meta))
        .ok_or("CLI history state is missing or ambiguous; refusing a fresh conversation")?;
    let source_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid CLI session directory")?
        .to_string();
    prepare_directory(
        meta,
        runtime_home,
        cwd,
        model_id,
        directory,
        &source_id,
        is_execution,
    )
}

fn prepare_directory(
    meta: &SessionMeta,
    runtime_home: &Path,
    cwd: &Path,
    model_id: &str,
    directory: PathBuf,
    source_id: &str,
    is_execution: bool,
) -> Result<PreparedContinuation, String> {
    let source = meta
        .cli_source
        .as_ref()
        .ok_or("Missing CLI history origin")?;
    let existing_home = directory
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent);
    let same_home = fs::canonicalize(runtime_home).ok().as_deref() == existing_home;
    let mut prepared = if is_execution && same_home && directory.join(ORIGIN_FILE).exists() {
        let marker: Value = serde_json::from_slice(
            &read_regular(&directory.join(ORIGIN_FILE), MAX_BYTES).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if marker.get("appSessionId").and_then(Value::as_str) != Some(meta.id.as_str()) {
            return Err("CLI execution state belongs to another App session".into());
        }
        reject_terminal_owner(runtime_home, source_id).map_err(|e| e.to_string())?;
        PreparedContinuation {
            agent_session_id: source_id.to_string(),
            directory,
            source_digest: marker
                .get("sourceDigest")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            lease: None,
        }
    } else {
        fork_state(
            &directory,
            source_id,
            source,
            &meta.id,
            runtime_home,
            cwd,
            model_id,
        )
        .map_err(|e| format!("CLI history continuation: {e}"))?
    };
    let mut lock_options = OpenOptions::new();
    lock_options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let lock = lock_options
        .open(prepared.directory.join("app_execution.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "CLI execution is already owned by another App process".to_string())?;
    prepared.lease = Some(lock);
    if !model_id.is_empty() {
        let path = prepared.directory.join("summary.json");
        let mut summary: Value =
            serde_json::from_slice(&read_regular(&path, MAX_BYTES).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if summary.pointer("/info/cwd").and_then(Value::as_str) != cwd.to_str() {
            return Err(
                "CLI execution cwd changed; refusing to load a different conversation".into(),
            );
        }
        summary["current_model_id"] = json!(model_id);
        crate::store_lock::write_bytes_atomic(
            &path,
            &serde_json::to_vec(&summary).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(prepared)
}

/// Persist the execution home independently of inference settings and provenance.
pub fn record_execution(meta: &SessionMeta, prepared: &PreparedContinuation) -> Result<(), String> {
    let mut execution = meta.cli_source.clone().ok_or("Missing CLI origin")?;
    let home = prepared
        .directory
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid execution path")?;
    execution.source_home = home.to_string_lossy().into_owned();
    execution.relative_dir = prepared
        .directory
        .strip_prefix(home)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    execution.agent_session_id = prepared.agent_session_id.clone();
    execution.app_owned = true;
    execution.revision = prepared.source_digest.clone();
    crate::store_lock::write_bytes_atomic(
        &crate::paths::session_dir(&meta.id).join("cli_execution.json"),
        &serde_json::to_vec(&execution).map_err(|e| e.to_string())?,
    )
}

fn reject_terminal_owner(home: &Path, id: &str) -> std::io::Result<()> {
    let bytes = match fs::read(home.join("active_sessions.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let entries: Vec<Value> = serde_json::from_slice(&bytes)?;
    if entries
        .iter()
        .any(|entry| entry.get("session_id").and_then(Value::as_str) == Some(id))
    {
        return Err(invalid(
            "This execution is registered to a terminal; close it before continuing in App",
        ));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    fs::create_dir(path)
}

fn child_dir(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    let path = parent.join(name);
    match private_dir(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(invalid("Symlink or non-directory in runtime path"));
    }
    Ok(path)
}

fn read_regular(path: &Path, remaining: u64) -> std::io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(invalid("Session state contains a symlink or special file"));
    }
    if meta.len() > remaining {
        return Err(invalid("Session state exceeds the safe copy limit"));
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("Session state is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(remaining + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > remaining {
        return Err(invalid("Session grew beyond the safe copy limit"));
    }
    Ok(bytes)
}

fn snapshot(root: &Path) -> std::io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(
        root: &Path,
        relative: &Path,
        depth: usize,
        total: &mut u64,
        entries: &mut usize,
        out: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> std::io::Result<()> {
        *entries += 1;
        if *entries > MAX_FILES {
            return Err(invalid("Too many session state entries"));
        }
        let path = root.join(relative);
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && depth == 0 => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            return Err(invalid("Session state contains a symlink"));
        }
        if meta.is_dir() {
            if depth >= 6 {
                return Err(invalid("Session state is nested too deeply"));
            }
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_text = name.to_string_lossy();
                if name_text.ends_with(".lock")
                    || name_text.ends_with(".tmp")
                    || name_text == "auth.json"
                    || name_text == "config.toml"
                {
                    continue;
                }
                visit(root, &relative.join(name), depth + 1, total, entries, out)?;
            }
        } else {
            if out.len() >= MAX_FILES {
                return Err(invalid("Too many session state files"));
            }
            let bytes = read_regular(&path, MAX_BYTES - *total)?;
            *total += bytes.len() as u64;
            out.insert(relative.to_owned(), bytes);
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    let mut total = 0;
    let mut entries = 0;
    for name in FILES.iter().chain(DIRECTORIES) {
        visit(root, Path::new(name), 0, &mut total, &mut entries, &mut out)?;
    }
    Ok(out)
}

fn digest(files: &BTreeMap<PathBuf, Vec<u8>>) -> String {
    let mut hash = Sha256::new();
    for (path, bytes) in files {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn optional_field(item: &Value, key: &str, valid: impl FnOnce(&Value) -> bool) -> bool {
    item.get(key)
        .is_none_or(|value| value.is_null() || valid(value))
}

fn valid_content_parts(value: &Value) -> bool {
    value.as_array().is_some_and(|parts| {
        parts
            .iter()
            .all(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").is_some_and(Value::is_string),
                Some("image") => part.get("url").is_some_and(Value::is_string),
                _ => false,
            })
    })
}

fn validate_chat(bytes: &[u8], version: u64) -> std::io::Result<()> {
    for line in bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        let item: Value = serde_json::from_slice(line)?;
        let valid = if version == 0 {
            matches!(
                item.get("role").and_then(Value::as_str),
                Some("system" | "user" | "assistant" | "tool" | "function")
            )
        } else {
            match item.get("type").and_then(Value::as_str) {
                Some("system") => {
                    item.get("content").is_some_and(Value::is_string)
                        && optional_field(&item, "synthetic_reason", Value::is_string)
                }
                Some("assistant") => {
                    item.get("content").is_some_and(Value::is_string)
                        && item.get("tool_calls").is_none_or(|calls| {
                            calls.as_array().is_some_and(|calls| {
                                calls.iter().all(|call| {
                                    ["id", "name", "arguments"]
                                        .iter()
                                        .all(|key| call.get(key).is_some_and(Value::is_string))
                                })
                            })
                        })
                        && ["model_id", "model_fingerprint", "system_fingerprint"]
                            .iter()
                            .all(|key| optional_field(&item, key, Value::is_string))
                        && optional_field(&item, "reasoning_effort", |value| {
                            matches!(
                                value.as_str(),
                                Some(
                                    "none"
                                        | "minimal"
                                        | "low"
                                        | "medium"
                                        | "high"
                                        | "xhigh"
                                        | "max"
                                )
                            )
                        })
                }
                Some("user") => {
                    item.get("content").is_some_and(valid_content_parts)
                        && ["synthetic_reason", "prior_turn_interrupt"]
                            .iter()
                            .all(|key| optional_field(&item, key, Value::is_string))
                        && ["cwd_generation", "prompt_index"]
                            .iter()
                            .all(|key| optional_field(&item, key, Value::is_u64))
                }
                Some("tool_result") => {
                    item.get("content").is_some_and(Value::is_string)
                        && item.get("tool_call_id").is_some_and(Value::is_string)
                        && item.get("images").is_none_or(valid_content_parts)
                }
                Some("backend_tool_call") => item.get("kind").is_some_and(Value::is_object),
                Some("reasoning") => item.get("id").is_some_and(Value::is_string),
                _ => false,
            }
        };
        if !valid {
            return Err(invalid(
                "Unsupported or corrupt native conversation item; refusing partial context",
            ));
        }
    }
    Ok(())
}

fn restamp_jsonl(bytes: &[u8], id: Option<&str>) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    for line in bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        let mut value: Value = serde_json::from_slice(line)
            .map_err(|e| invalid(format!("Incomplete or corrupt native transcript: {e}")))?;
        if let Some(id) = id {
            let notification = if value.get("method").is_some() {
                value.get_mut("params")
            } else {
                Some(&mut value)
            };
            let fields = notification
                .and_then(Value::as_object_mut)
                .ok_or_else(|| invalid("Invalid native update envelope"))?;
            fields.insert("sessionId".into(), json!(id));
        }
        serde_json::to_writer(&mut out, &value)?;
        out.push(b'\n');
    }
    Ok(out)
}

fn fork_state(
    source_dir: &Path,
    source_id: &str,
    origin: &CliSessionSource,
    app_id: &str,
    runtime_home: &Path,
    cwd: &Path,
    model: &str,
) -> std::io::Result<PreparedContinuation> {
    let mut files = snapshot(source_dir)?;
    let source_digest = digest(&files);
    if source_digest != digest(&snapshot(source_dir)?) {
        return Err(invalid(
            "CLI history changed during snapshot; retry when the terminal turn has finished",
        ));
    }
    let summary_bytes = files
        .get(Path::new("summary.json"))
        .ok_or_else(|| invalid("Native summary.json is missing"))?;
    let mut summary: Value = serde_json::from_slice(summary_bytes)?;
    if summary.pointer("/info/id").and_then(Value::as_str) != Some(source_id) {
        return Err(invalid(
            "Native session identity does not match its directory",
        ));
    }
    let cwd_text = cwd
        .to_str()
        .ok_or_else(|| invalid("Non-UTF8 session cwd"))?;
    if summary.pointer("/info/cwd").and_then(Value::as_str) != Some(cwd_text) {
        return Err(invalid(
            "CLI history must continue in its original working directory",
        ));
    }
    if summary
        .get("chat_format_version")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 1
    {
        return Err(invalid("Unsupported native conversation format"));
    }
    let chat = files.get(Path::new("chat_history.jsonl")).ok_or_else(|| {
        invalid("Native chat_history.jsonl is missing; refusing lossy transcript bootstrap")
    })?;
    validate_chat(
        chat,
        summary
            .get("chat_format_version")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )?;
    if chat.iter().all(u8::is_ascii_whitespace)
        && summary
            .get("num_chat_messages")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
    {
        return Err(invalid(
            "Native conversation is empty but its summary contains history",
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    summary["info"]["id"] = json!(id);
    summary["parent_session_id"] = json!(source_id);
    summary["forked_at"] = json!(chrono::Utc::now().to_rfc3339());
    summary["created_at"] = summary["forked_at"].clone();
    summary["session_kind"] = json!("fork");
    summary["grok_home"] = json!(runtime_home.to_string_lossy());
    summary["next_trace_turn"] = json!(0);
    // CLI load derives fresh runtime identities when these are absent.
    for key in [
        "agent_id",
        "attempt_id",
        "collection_id",
        "request_id",
        "pending_cwd_switch_reminder",
        "hidden",
    ] {
        summary
            .as_object_mut()
            .ok_or_else(|| invalid("Invalid native summary"))?
            .remove(key);
    }
    if !model.is_empty() {
        summary["current_model_id"] = json!(model);
    }
    files.insert(
        PathBuf::from("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    );
    if let Some(updates) = files.get_mut(Path::new("updates.jsonl")) {
        *updates = restamp_jsonl(updates, Some(&id))?;
    }
    if let Some(usage) = files.get_mut(Path::new("usage.json")) {
        let mut value: Value = serde_json::from_slice(usage)?;
        value["session_id"] = json!(id);
        *usage = serde_json::to_vec(&value)?;
    }
    files.insert(
        PathBuf::from(ORIGIN_FILE),
        serde_json::to_vec_pretty(&json!({
            "version": 1, "appSessionId": app_id, "origin": origin,
            "parentSessionId": source_id, "parentDirectory": source_dir,
            "sourceDigest": source_digest, "executionSessionId": id,
        }))?,
    );

    fs::create_dir_all(runtime_home)?;
    let home = fs::canonicalize(runtime_home)?;
    let sessions = child_dir(&home, "sessions")?;
    let encoded =
        percent_encoding::utf8_percent_encode(cwd_text, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    // CLI's encoding leaves RFC3986 unreserved characters unchanged.
    let encoded = encoded
        .replace("%2D", "-")
        .replace("%2E", ".")
        .replace("%5F", "_")
        .replace("%7E", "~");
    let encoded = if encoded.len() > 255 {
        let parent = source_dir
            .parent()
            .ok_or_else(|| invalid("Missing source cwd directory"))?;
        if read_regular(&parent.join(".cwd"), MAX_BYTES)? != cwd_text.as_bytes() {
            return Err(invalid(
                "Long-path CLI cwd marker does not match the session",
            ));
        }
        parent
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("Invalid CLI cwd directory"))?
            .to_string()
    } else {
        encoded
    };
    let parent = child_dir(&sessions, &encoded)?;
    if !encoded.starts_with('%') {
        let marker = parent.join(".cwd");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
        {
            Ok(mut file) => {
                file.write_all(cwd_text.as_bytes())?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_regular(&marker, MAX_BYTES)? != cwd_text.as_bytes() {
                    return Err(invalid("Runtime cwd marker collision"));
                }
            }
            Err(e) => return Err(e),
        }
    }
    let stage = parent.join(format!(".app-continue-{id}"));
    private_dir(&stage)?;
    let result = (|| {
        for (relative, bytes) in &files {
            let mut directory = stage.clone();
            if let Some(parent) = relative.parent() {
                for component in parent.components() {
                    let Component::Normal(name) = component else {
                        return Err(invalid("Invalid state path"));
                    };
                    directory = child_dir(
                        &directory,
                        name.to_str()
                            .ok_or_else(|| invalid("Non-UTF8 state path"))?,
                    )?;
                }
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(stage.join(relative))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        let target = parent.join(&id);
        publish_no_replace(&stage, &target)?;
        Ok(PreparedContinuation {
            agent_session_id: id,
            directory: target,
            source_digest,
            lease: None,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}

fn publish_no_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        fs::rename(source, target)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (source, target);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Atomic no-replace directory publication is unavailable",
        ))
    }
}

#[cfg(test)]
mod tests;
