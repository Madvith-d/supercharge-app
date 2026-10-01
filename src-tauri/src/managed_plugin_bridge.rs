//! Session-owned Plugin API MCP bridge bindings.
//!
//! The management bearer stays in [`crate::plugin_api::PluginApiConnection`]. A
//! separately scoped read/execute credential is written only to an owner-only
//! file consumed by the stdio bridge. Public DTOs never contain either secret.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const BRIDGE_VERSION: &str = "0.1.0";
const BRIDGE_SERVER_PREFIX: &str = "supercharge-plugin-api";
const EMBEDDED_BRIDGE: &[u8] = include_bytes!("../resources/plugin-bridge-0.1.0.mjs");
const EMBEDDED_BRIDGE_SHA256: &str = include_str!("../resources/plugin-bridge-0.1.0.mjs.sha256");
static STORE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeLifecycleStatus {
    Saved,
    Waiting,
    Applied,
    Ready,
    Failed,
    Removing,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeToolSummary {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeStateFile {
    schema_version: u32,
    revision: u64,
    app_session_id: String,
    plugin_id: String,
    server_id: String,
    server_name: String,
    workspace_id: String,
    release_digest: String,
    endpoint: String,
    generation: String,
    launch_slot: String,
    agent_session_id: Option<String>,
    node_path: String,
    policy_expires_at: String,
    status: BridgeLifecycleStatus,
    updated_at: String,
    last_error: Option<String>,
    tools: Vec<BridgeToolSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub configured: bool,
    pub revision: Option<u64>,
    pub app_session_id: String,
    pub plugin_id: Option<String>,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub workspace_id: Option<String>,
    pub release_digest: Option<String>,
    pub endpoint: Option<String>,
    pub generation: Option<String>,
    pub agent_session_id: Option<String>,
    pub policy_expires_at: Option<String>,
    pub status: BridgeLifecycleStatus,
    pub updated_at: Option<String>,
    pub last_error: Option<String>,
    pub tools: Vec<BridgeToolSummary>,
}

#[derive(Debug)]
pub struct StageBinding<'a> {
    pub expected_revision: Option<u64>,
    pub app_session_id: &'a str,
    pub plugin_id: &'a str,
    pub server_id: &'a str,
    pub workspace_id: &'a str,
    pub release_digest: &'a str,
    pub endpoint: &'a str,
    pub bridge_token: &'a str,
    pub agent_session_id: Option<&'a str>,
    pub node_path: &'a Path,
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(session_id)
        .map(|_| ())
        .map_err(|_| "Invalid app session ID".to_string())
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value.starts_with(|character: char| character.is_ascii_lowercase())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!("Invalid {label}"));
    }
    Ok(())
}

fn bridge_root() -> PathBuf {
    crate::paths::app_data_root()
        .join("plugin-api")
        .join("bridges")
}

fn session_root(session_id: &str) -> Result<PathBuf, String> {
    validate_session_id(session_id)?;
    Ok(bridge_root().join(session_id))
}

fn state_path(session_id: &str) -> Result<PathBuf, String> {
    Ok(session_root(session_id)?.join("state.json"))
}

fn generation_root(session_id: &str, generation: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(generation).map_err(|_| "Invalid managed MCP generation")?;
    Ok(session_root(session_id)?
        .join("generations")
        .join(generation))
}

fn binding_path(session_id: &str, generation: &str) -> Result<PathBuf, String> {
    Ok(generation_root(session_id, generation)?.join("binding.json"))
}

fn token_path(session_id: &str, generation: &str) -> Result<PathBuf, String> {
    Ok(generation_root(session_id, generation)?.join("credential"))
}

fn artifact_path() -> PathBuf {
    bridge_root()
        .join("artifacts")
        .join(format!("plugin-bridge-{BRIDGE_VERSION}.mjs"))
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).map_err(|error| format!("create bridge directory: {error}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect bridge directory: {error}"))
}

#[cfg(not(unix))]
fn private_directory(_path: &Path) -> Result<(), String> {
    Err("Managed MCP bridge is supported only on Linux".into())
}

#[cfg(unix)]
fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let parent = path.parent().ok_or("Bridge file has no parent")?;
    private_directory(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bridge"),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create private bridge file: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("write private bridge file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync private bridge file: {error}"))?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("protect private bridge file: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("replace private bridge file: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(unix))]
fn write_private_atomic(_path: &Path, _bytes: &[u8]) -> Result<(), String> {
    Err("Managed MCP bridge is supported only on Linux".into())
}

fn embedded_digest() -> String {
    hex::encode(Sha256::digest(EMBEDDED_BRIDGE))
}

fn expected_embedded_digest() -> Result<&'static str, String> {
    let expected = EMBEDDED_BRIDGE_SHA256
        .split_ascii_whitespace()
        .next()
        .ok_or("Embedded bridge checksum is missing")?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Embedded bridge checksum is invalid".into());
    }
    Ok(expected)
}

pub fn ensure_bridge_artifact() -> Result<PathBuf, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let expected = expected_embedded_digest()?;
    if embedded_digest() != expected {
        return Err("Embedded bridge artifact failed integrity verification".into());
    }
    let path = artifact_path();
    if let Ok(bytes) = fs::read(&path) {
        if hex::encode(Sha256::digest(bytes)) == expected {
            return Ok(path);
        }
    }
    write_private_atomic(&path, EMBEDDED_BRIDGE)?;
    Ok(path)
}

fn read_state_unlocked(session_id: &str) -> Result<Option<BridgeStateFile>, String> {
    let path = state_path(session_id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read managed MCP state: {error}")),
    };
    let state: BridgeStateFile =
        serde_json::from_slice(&bytes).map_err(|_| "Managed MCP state is invalid")?;
    if state.schema_version != 1 || state.app_session_id != session_id {
        return Err("Managed MCP state identity is invalid".into());
    }
    Ok(Some(state))
}

fn public_status(session_id: &str, state: Option<BridgeStateFile>) -> BridgeStatus {
    match state {
        Some(state) => BridgeStatus {
            configured: !matches!(state.status, BridgeLifecycleStatus::Removed),
            revision: Some(state.revision),
            app_session_id: session_id.to_string(),
            plugin_id: Some(state.plugin_id),
            server_id: Some(state.server_id),
            server_name: Some(state.server_name),
            workspace_id: Some(state.workspace_id),
            release_digest: Some(state.release_digest),
            endpoint: Some(state.endpoint),
            generation: Some(state.generation),
            agent_session_id: state.agent_session_id,
            policy_expires_at: Some(state.policy_expires_at),
            status: state.status,
            updated_at: Some(state.updated_at),
            last_error: state.last_error,
            tools: state.tools,
        },
        None => BridgeStatus {
            configured: false,
            revision: None,
            app_session_id: session_id.to_string(),
            plugin_id: None,
            server_id: None,
            server_name: None,
            workspace_id: None,
            release_digest: None,
            endpoint: None,
            generation: None,
            agent_session_id: None,
            policy_expires_at: None,
            status: BridgeLifecycleStatus::Removed,
            updated_at: None,
            last_error: None,
            tools: Vec::new(),
        },
    }
}

pub fn status(session_id: &str) -> Result<BridgeStatus, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(public_status(session_id, read_state_unlocked(session_id)?))
}

fn server_name(session_id: &str, generation: &str) -> String {
    let compact = |value: &str, take: usize| {
        value
            .chars()
            .filter(|character| character.is_ascii_hexdigit())
            .take(take)
            .collect::<String>()
    };
    format!(
        "{BRIDGE_SERVER_PREFIX}-{}-{}",
        compact(session_id, 8),
        compact(generation, 8)
    )
}

fn binding_json(state: &BridgeStateFile, token_file: &Path) -> Value {
    let mut target = json!({
        "kind": "desktop",
        "owner": format!("uid:{}", current_uid().unwrap_or_default()),
        "id": state.app_session_id,
        "generation": state.generation,
        "launchSlot": state.launch_slot,
    });
    if let Some(agent_session_id) = state.agent_session_id.as_ref() {
        target["agentSessionId"] = Value::String(agent_session_id.clone());
    }
    json!({
        "schemaVersion": 1,
        "revision": state.revision,
        "target": target,
        "baseUrl": state.endpoint,
        "tokenFile": token_file,
        "workspaceId": state.workspace_id,
        "pluginDigests": { state.plugin_id.clone(): state.release_digest },
        "policy": {
            "expiresAt": state.policy_expires_at,
            "servers": [{
                "pluginId": state.plugin_id,
                "serverId": state.server_id,
                "releaseDigest": state.release_digest,
            }]
        }
    })
}

pub fn stage(input: StageBinding<'_>) -> Result<BridgeStatus, String> {
    validate_session_id(input.app_session_id)?;
    validate_component(input.plugin_id, "plugin ID")?;
    validate_component(input.server_id, "server ID")?;
    validate_component(input.workspace_id, "workspace ID")?;
    if input.bridge_token.len() < 32
        || input.bridge_token.len() > 256
        || input.bridge_token.chars().any(char::is_whitespace)
    {
        return Err("Bridge key must contain 32–256 characters without whitespace".into());
    }
    let node_path = input
        .node_path
        .canonicalize()
        .map_err(|error| format!("resolve Node runtime: {error}"))?;
    if !node_path.is_absolute() || !node_path.is_file() {
        return Err("Node runtime path is invalid".into());
    }
    let _ = ensure_bridge_artifact()?;
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = read_state_unlocked(input.app_session_id)?;
    match (&previous, input.expected_revision) {
        (None, None) => {}
        (Some(current), Some(expected)) if current.revision == expected => {}
        _ => return Err("Managed MCP binding revision changed; refresh before retrying".into()),
    }
    let revision = previous
        .as_ref()
        .map(|current| current.revision.saturating_add(1))
        .unwrap_or(1);
    let root = session_root(input.app_session_id)?;
    private_directory(&bridge_root())?;
    private_directory(&root)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let generation_dir = generation_root(input.app_session_id, &generation)?;
    private_directory(&root.join("generations"))?;
    private_directory(&generation_dir)?;
    let token_file = token_path(input.app_session_id, &generation)?;
    write_private_atomic(&token_file, input.bridge_token.as_bytes())?;
    let state = BridgeStateFile {
        schema_version: 1,
        revision,
        app_session_id: input.app_session_id.to_string(),
        plugin_id: input.plugin_id.to_string(),
        server_id: input.server_id.to_string(),
        server_name: server_name(input.app_session_id, &generation),
        workspace_id: input.workspace_id.to_string(),
        release_digest: input.release_digest.to_string(),
        endpoint: input.endpoint.to_string(),
        generation,
        launch_slot: uuid::Uuid::new_v4().to_string(),
        agent_session_id: input.agent_session_id.map(str::to_string),
        node_path: node_path.to_string_lossy().to_string(),
        policy_expires_at: (Utc::now() + ChronoDuration::hours(24))
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        status: BridgeLifecycleStatus::Saved,
        updated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        last_error: None,
        tools: Vec::new(),
    };
    let binding = serde_json::to_vec_pretty(&binding_json(&state, &token_file))
        .map_err(|error| error.to_string())?;
    write_private_atomic(
        &binding_path(input.app_session_id, &state.generation)?,
        &binding,
    )?;
    let state_bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    write_private_atomic(&state_path(input.app_session_id)?, &state_bytes)?;
    Ok(public_status(input.app_session_id, Some(state)))
}

pub fn bind_agent_session(
    session_id: &str,
    agent_session_id: &str,
) -> Result<BridgeStatus, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state_unlocked(session_id)?.ok_or("Managed MCP binding not found")?;
    state.agent_session_id = Some(agent_session_id.to_string());
    state.status = BridgeLifecycleStatus::Waiting;
    state.tools.clear();
    state.last_error = None;
    state.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let token_file = token_path(session_id, &state.generation)?;
    let binding = serde_json::to_vec_pretty(&binding_json(&state, &token_file))
        .map_err(|error| error.to_string())?;
    write_private_atomic(&binding_path(session_id, &state.generation)?, &binding)?;
    let state_bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    write_private_atomic(&state_path(session_id)?, &state_bytes)?;
    Ok(public_status(session_id, Some(state)))
}

pub fn fail(session_id: &str, error: String) {
    let Ok(generation) = state_generation(session_id) else {
        return;
    };
    let _ = set_status(
        session_id,
        &generation,
        BridgeLifecycleStatus::Failed,
        None,
        Vec::new(),
        Some(error),
    );
}

pub fn set_status(
    session_id: &str,
    generation: &str,
    status: BridgeLifecycleStatus,
    agent_session_id: Option<&str>,
    tools: Vec<BridgeToolSummary>,
    error: Option<String>,
) -> Result<BridgeStatus, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state_unlocked(session_id)?.ok_or("Managed MCP binding not found")?;
    if state.generation != generation {
        return Err("Managed MCP binding changed during application".into());
    }
    state.status = status;
    state.agent_session_id = agent_session_id
        .map(str::to_string)
        .or(state.agent_session_id);
    state.tools = tools;
    state.last_error = error.map(|value| value.chars().take(240).collect());
    state.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    write_private_atomic(&state_path(session_id)?, &bytes)?;
    Ok(public_status(session_id, Some(state)))
}

pub fn prepare_for_launch(session_id: &str) -> Result<bool, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = session_id;
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    {
        let _guard = STORE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(mut state) = read_state_unlocked(session_id)? else {
            return Ok(false);
        };
        if matches!(
            state.status,
            BridgeLifecycleStatus::Removing | BridgeLifecycleStatus::Removed
        ) || DateTimeCheck::expired(&state.policy_expires_at)
        {
            return Ok(false);
        }
        state.launch_slot = uuid::Uuid::new_v4().to_string();
        state.agent_session_id = None;
        state.status = BridgeLifecycleStatus::Waiting;
        state.tools.clear();
        state.last_error = None;
        state.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let token_file = token_path(session_id, &state.generation)?;
        if !token_file.is_file() {
            return Err("Managed MCP bridge credential is unavailable; apply again".into());
        }
        let binding = serde_json::to_vec_pretty(&binding_json(&state, &token_file))
            .map_err(|error| error.to_string())?;
        write_private_atomic(&binding_path(session_id, &state.generation)?, &binding)?;
        let state_bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
        write_private_atomic(&state_path(session_id)?, &state_bytes)?;
        Ok(true)
    }
}

pub fn entry_for_session(session_id: &str) -> Result<Option<Value>, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = session_id;
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
        let _guard = STORE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(state) = read_state_unlocked(session_id)? else {
            return Ok(None);
        };
        if !matches!(
            state.status,
            BridgeLifecycleStatus::Saved
                | BridgeLifecycleStatus::Waiting
                | BridgeLifecycleStatus::Applied
                | BridgeLifecycleStatus::Ready
                | BridgeLifecycleStatus::Failed
                | BridgeLifecycleStatus::Removing
        ) {
            return Ok(None);
        }
        if DateTimeCheck::expired(&state.policy_expires_at) {
            return Ok(None);
        }
        let artifact = ensure_bridge_artifact_unlocked()?;
        let binding = binding_path(session_id, &state.generation)?;
        if !binding.is_file() || !token_path(session_id, &state.generation)?.is_file() {
            return Ok(None);
        }
        Ok(Some(json!({
            "name": state.server_name,
            "command": state.node_path,
            "args": [artifact],
            "env": [{
                "name": "SUPERCHARGE_PLUGIN_BRIDGE_BINDING",
                "value": binding,
            }],
        })))
    }
}

struct DateTimeCheck;
impl DateTimeCheck {
    fn expired(value: &str) -> bool {
        chrono::DateTime::parse_from_rfc3339(value)
            .map(|time| time.with_timezone(&Utc) <= Utc::now())
            .unwrap_or(true)
    }
}

fn ensure_bridge_artifact_unlocked() -> Result<PathBuf, String> {
    let expected = expected_embedded_digest()?;
    if embedded_digest() != expected {
        return Err("Embedded bridge artifact failed integrity verification".into());
    }
    let path = artifact_path();
    if let Ok(bytes) = fs::read(&path) {
        if hex::encode(Sha256::digest(bytes)) == expected {
            return Ok(path);
        }
    }
    write_private_atomic(&path, EMBEDDED_BRIDGE)?;
    Ok(path)
}

pub fn begin_remove(session_id: &str, expected_revision: u64) -> Result<BridgeStatus, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state_unlocked(session_id)?.ok_or("Managed MCP binding not found")?;
    if state.revision != expected_revision {
        return Err("Managed MCP binding revision changed; refresh before retrying".into());
    }
    state.revision = state.revision.saturating_add(1);
    state.status = BridgeLifecycleStatus::Removing;
    state.tools.clear();
    state.last_error = None;
    state.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    write_private_atomic(&state_path(session_id)?, &bytes)?;
    Ok(public_status(session_id, Some(state)))
}

pub fn finish_remove(session_id: &str) -> Result<BridgeStatus, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = session_root(session_id)?;
    let _ = fs::remove_dir_all(root);
    Ok(public_status(session_id, None))
}

pub fn cleanup_old_generations(session_id: &str, keep_generation: &str) {
    let Ok(root) = session_root(session_id).map(|path| path.join("generations")) else {
        return;
    };
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy() != keep_generation {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub fn forget_session(session_id: &str) {
    if let Ok(root) = session_root(session_id) {
        let _guard = STORE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(target_os = "linux")]
pub fn current_uid() -> Option<u32> {
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(target_os = "linux"))]
pub fn current_uid() -> Option<u32> {
    None
}

fn supported_node_version(raw: &str) -> bool {
    let token = raw.trim().trim_start_matches('v');
    let mut parts = token.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
    matches!((major, minor), (Some(22), Some(minor)) if minor >= 16)
        || matches!(major, Some(major) if major >= 24)
}

pub async fn resolve_node_runtime() -> Result<PathBuf, String> {
    #[cfg(not(target_os = "linux"))]
    {
        return Err("Managed MCP bridge is supported only on Linux".into());
    }
    #[cfg(target_os = "linux")]
    {
        let path = which::which("node").map_err(|_| "Node.js 22.16+ or 24+ is required")?;
        let mut command = crate::process_util::tokio_command(&path);
        command.arg("--version");
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
            .await
            .map_err(|_| "Node.js version check timed out")?
            .map_err(|error| format!("Node.js version check failed: {error}"))?;
        let version =
            String::from_utf8(output.stdout).map_err(|_| "Node.js returned an invalid version")?;
        if !output.status.success() || !supported_node_version(&version) {
            return Err(
                "Managed MCP requires Node.js 22.16+ in the Node 22 line, or Node.js 24+".into(),
            );
        }
        path.canonicalize()
            .map_err(|error| format!("resolve Node.js path: {error}"))
    }
}

pub fn state_generation(session_id: &str) -> Result<String, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_state_unlocked(session_id)?
        .map(|state| state.generation)
        .ok_or_else(|| "Managed MCP binding not found".into())
}

pub fn server_name_for_session(session_id: &str) -> Result<String, String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_state_unlocked(session_id)?
        .map(|state| state.server_name)
        .ok_or_else(|| "Managed MCP binding not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_are_session_scoped_and_safe() {
        let a = server_name(
            "11111111-1111-4111-8111-111111111111",
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        );
        let b = server_name(
            "22222222-2222-4222-8222-222222222222",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        );
        assert_ne!(a, b);
        assert!(a.starts_with(BRIDGE_SERVER_PREFIX));
        assert!(a
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
    }

    #[test]
    fn embedded_artifact_matches_pinned_checksum() {
        assert_eq!(embedded_digest(), expected_embedded_digest().unwrap());
    }

    #[test]
    fn rejects_unsafe_identifiers() {
        assert!(validate_component("valid-id", "id").is_ok());
        assert!(validate_component("../escape", "id").is_err());
        assert!(validate_component("Upper", "id").is_err());
        assert!(validate_session_id("../escape").is_err());
    }

    #[test]
    fn enforces_supported_node_versions() {
        assert!(supported_node_version("v22.16.0"));
        assert!(supported_node_version("24.0.0"));
        assert!(supported_node_version("v25.3.1"));
        assert!(!supported_node_version("v22.15.9"));
        assert!(!supported_node_version("v23.9.0"));
        assert!(!supported_node_version("garbage"));
    }
}
