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
