#[tauri::command]
pub async fn plugin_api_catalog() -> Result<serde_json::Value, String> {
    let url = "https://raw.githubusercontent.com/iotserver24/supercharge-app/28a365592f2271a88909178621000b24e5373157/public/managed-mcp/catalog.json";
    let response = reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "Managed MCP catalog is unavailable".to_owned())?
        .error_for_status()
        .map_err(|_| "Managed MCP catalog is unavailable".to_owned())?;
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Managed MCP catalog is unavailable".to_owned())?;
    if bytes.len() > 256 * 1024 {
        return Err("Managed MCP catalog exceeds 256 KiB".into());
    }
    let catalog: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Managed MCP catalog is invalid".to_owned())?;
    if catalog.get("schemaVersion").and_then(serde_json::Value::as_u64) != Some(1)
        || catalog.get("plugins").and_then(serde_json::Value::as_array).is_none()
    {
        return Err("Managed MCP catalog is invalid".into());
    }
    let mut catalog = catalog;
    let entries = catalog
        .get_mut("plugins")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "Managed MCP catalog is invalid".to_owned())?;
    for entry in entries {
        let id = entry.get("id").and_then(serde_json::Value::as_str);
        let version = entry.get("version").and_then(serde_json::Value::as_str);
        if id != Some("vectra-echo") || version != Some("1.0.0") {
            return Err("Unreviewed Managed MCP catalog entry".into());
        }
        let manifest_url = "https://raw.githubusercontent.com/iotserver24/supercharge-app/6204686983497b32b798376b6ea6ed5e105be182/public/managed-mcp/echo-1.0.0/plugin.json";
        let manifest = reqwest::Client::new()
            .get(manifest_url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| "Reviewed Managed MCP manifest is unavailable".to_owned())?
            .error_for_status()
            .map_err(|_| "Reviewed Managed MCP manifest is unavailable".to_owned())?
            .bytes()
            .await
            .map_err(|_| "Reviewed Managed MCP manifest is unavailable".to_owned())?;
        if manifest.len() > 64 * 1024 {
            return Err("Reviewed Managed MCP manifest is too large".into());
        }
        let manifest: serde_json::Value = serde_json::from_slice(&manifest)
            .map_err(|_| "Reviewed Managed MCP manifest is invalid".to_owned())?;
        if manifest.get("id").and_then(serde_json::Value::as_str) != id
            || manifest.get("version").and_then(serde_json::Value::as_str) != version
        {
            return Err("Reviewed Managed MCP manifest identity mismatch".into());
        }
        entry["manifest"] = manifest;
    }
    Ok(catalog)
}

#[tauri::command]
pub fn plugin_api_status(
    connection: State<'_, crate::plugin_api::PluginApiConnection>,
) -> crate::plugin_api::ConnectionStatus {
    connection.status()
}

#[tauri::command]
pub async fn plugin_api_connect(
    connection: State<'_, crate::plugin_api::PluginApiConnection>,
    endpoint: String,
    api_key: String,
) -> Result<crate::plugin_api::ConnectionStatus, String> {
    connection.connect(&endpoint, api_key).await
}

#[tauri::command]
pub fn plugin_api_disconnect(
    connection: State<'_, crate::plugin_api::PluginApiConnection>,
) -> crate::plugin_api::ConnectionStatus {
    connection.disconnect()
}

#[tauri::command]
pub async fn plugin_api_request(
    connection: State<'_, crate::plugin_api::PluginApiConnection>,
    connection_id: String,
    request: crate::plugin_api::PluginRequest,
) -> Result<serde_json::Value, String> {
    connection.execute(&connection_id, request).await
}

#[tauri::command]
pub fn plugin_api_bridge_status(
    app_session_id: String,
) -> Result<crate::managed_plugin_bridge::BridgeStatus, String> {
    crate::managed_plugin_bridge::status(&app_session_id)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn plugin_api_bridge_apply(
    mgr: State<'_, Arc<SessionManager>>,
    connection: State<'_, crate::plugin_api::PluginApiConnection>,
    connection_id: String,
    app_session_id: String,
    plugin_id: String,
    server_id: String,
    workspace_id: String,
    bridge_key: String,
    expected_revision: Option<u64>,
) -> Result<crate::managed_plugin_bridge::BridgeStatus, String> {
    let before = mgr.managed_plugin_target(&app_session_id)?;
    let plan = connection
        .prepare_bridge(
            &connection_id,
            &plugin_id,
            &server_id,
            &workspace_id,
            &before.workspace_path,
            bridge_key,
        )
        .await?;
    let after = mgr.managed_plugin_target(&app_session_id)?;
    if before.workspace_path != after.workspace_path
        || before.agent_session_id != after.agent_session_id
    {
        return Err("Selected agent target changed during Plugin API preflight".into());
    }
    let node_path = crate::managed_plugin_bridge::resolve_node_runtime().await?;
    crate::managed_plugin_bridge::stage(crate::managed_plugin_bridge::StageBinding {
        expected_revision,
        app_session_id: &app_session_id,
        plugin_id: &plan.plugin_id,
        server_id: &plan.server_id,
        workspace_id: &plan.workspace_id,
        release_digest: &plan.release_digest,
        endpoint: &plan.endpoint,
        bridge_token: &plan.bridge_token,
        agent_session_id: after.agent_session_id.as_deref(),
        node_path: &node_path,
    })?;
    mgr.apply_managed_plugin_bridge(&app_session_id).await
}

#[tauri::command]
pub async fn plugin_api_bridge_retry(
    mgr: State<'_, Arc<SessionManager>>,
    app_session_id: String,
    expected_revision: u64,
) -> Result<crate::managed_plugin_bridge::BridgeStatus, String> {
    let status = crate::managed_plugin_bridge::status(&app_session_id)?;
    if status.revision != Some(expected_revision) {
        return Err("Managed MCP binding revision changed; refresh before retrying".into());
    }
    mgr.retry_managed_plugin_bridge(&app_session_id).await
}

#[tauri::command]
pub async fn plugin_api_bridge_verify(
    mgr: State<'_, Arc<SessionManager>>,
    app_session_id: String,
    expected_revision: u64,
    tool: String,
    arguments: serde_json::Value,
    consent: bool,
) -> Result<crate::managed_plugin_bridge::BridgeStatus, String> {
    mgr.verify_managed_plugin_call(
        &app_session_id,
        expected_revision,
        &tool,
        arguments,
        consent,
    )
    .await
}

#[tauri::command]
pub async fn plugin_api_bridge_remove(
    mgr: State<'_, Arc<SessionManager>>,
    app_session_id: String,
    expected_revision: u64,
) -> Result<crate::managed_plugin_bridge::BridgeStatus, String> {
    mgr.remove_managed_plugin_bridge(&app_session_id, expected_revision)
        .await
}
