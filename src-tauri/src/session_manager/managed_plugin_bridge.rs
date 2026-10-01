use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tauri::AppHandle;

use crate::acp_client::AcpClient;
use crate::managed_plugin_bridge::{self, BridgeLifecycleStatus, BridgeStatus, BridgeToolSummary};
use crate::store;

use super::SessionManager;

pub(crate) struct ManagedPluginTarget {
    pub workspace_path: PathBuf,
    pub agent_session_id: Option<String>,
}

pub(super) struct AttachedManagedSession {
    pub(super) acp: Arc<AcpClient>,
    pub(super) agent_session_id: String,
    pub(super) project_path: String,
    pub(super) busy: bool,
}

fn persisted_workspace(app_session_id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(app_session_id).map_err(|_| "Invalid app session ID")?;
    let session = store::load_sessions_index()
        .into_iter()
        .find(|session| session.id == app_session_id)
        .ok_or("Selected session does not exist")?;
    if let Some(worktree) = session
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        return PathBuf::from(worktree)
            .canonicalize()
            .map_err(|_| "Selected session worktree is unavailable".into());
    }
    if let Some(project_id) = session.project_id.as_deref() {
        let project = store::load_projects()
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or("Selected session project is unavailable")?;
        if project.is_ssh_remote() {
            return Err("Managed MCP bridge is not supported for SSH sessions".into());
        }
        return PathBuf::from(project.path)
            .canonicalize()
            .map_err(|_| "Selected session workspace is unavailable".into());
    }
    store::ensure_general_workspace_dir()?
        .canonicalize()
        .map_err(|_| "Default session workspace is unavailable".into())
}

fn unwrap_extension_result(value: Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        if !error.is_null() {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("Agent extension request failed");
            return Err(message.chars().take(240).collect());
        }
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn discovered_bridge(
    value: Value,
    server_name: &str,
) -> Result<Option<Vec<BridgeToolSummary>>, String> {
    let result = unwrap_extension_result(value)?;
    let servers = result
        .get("servers")
        .and_then(Value::as_array)
        .ok_or("Agent returned an invalid MCP inventory")?;
    let Some(server) = servers
        .iter()
        .find(|server| server.get("name").and_then(Value::as_str) == Some(server_name))
    else {
        return Ok(None);
    };
    let session = server
        .get("session")
        .and_then(Value::as_object)
        .ok_or("Managed MCP was not discovered by the exact agent session")?;
    if session.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err("Managed MCP is disabled in the exact agent session".into());
    }
    match session.get("status").and_then(Value::as_str) {
        Some("ready") => {}
        Some("initializing") | None => return Ok(None),
        Some(status) => return Err(format!("Managed MCP session status is {status}")),
    }
    let tools = session
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("Managed MCP tool inventory is invalid")?
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.to_string();
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(BridgeToolSummary { name, description })
        })
        .collect();
    Ok(Some(tools))
}

impl SessionManager {
    fn attached_managed_session(&self, app_session_id: &str) -> Option<AttachedManagedSession> {
        {
            let guard = self.inner.lock();
            if let Some(session) = guard.as_ref() {
                if session.app_session_id == app_session_id {
                    return Some(AttachedManagedSession {
                        acp: session.acp.as_ref()?.clone(),
                        agent_session_id: session.meta.agent_session_id.clone()?,
                        project_path: session.project_path.clone()?,
                        busy: Self::live_session_is_busy(session),
                    });
                }
            }
        }
        {
            let background = self.background.lock();
            if let Some(session) = background.get(app_session_id) {
                return Some(AttachedManagedSession {
                    acp: session.acp.as_ref()?.clone(),
                    agent_session_id: session.meta.agent_session_id.clone()?,
                    project_path: session.project_path.clone()?,
                    busy: Self::live_session_is_busy(session),
                });
            }
        }
        self.parked.lock().get(app_session_id).and_then(|session| {
            Some(AttachedManagedSession {
                acp: session.acp.clone(),
                agent_session_id: session.meta.agent_session_id.clone()?,
                project_path: session.project_path.clone()?,
                busy: false,
            })
        })
    }

    pub fn managed_plugin_target(
        &self,
        app_session_id: &str,
    ) -> Result<ManagedPluginTarget, String> {
        if !cfg!(target_os = "linux") {
            return Err("Managed MCP bridge is supported only for local Linux sessions".into());
        }
        let settings = store::load_settings();
        if settings
            .acp_server_addr
            .as_deref()
            .map(str::trim)
            .is_some_and(|address| !address.is_empty())
            || settings.cli_backend.eq_ignore_ascii_case("wsl")
        {
            return Err(
                "Managed MCP bridge is not supported for remote or WSL agent sessions".into(),
            );
        }
        if let Some(attached) = self.attached_managed_session(app_session_id) {
            if !attached.acp.owns_local_process_tree() {
                return Err("Managed MCP bridge requires a local Linux agent process".into());
            }
            let workspace_path = PathBuf::from(attached.project_path)
                .canonicalize()
                .map_err(|_| "Selected session workspace is unavailable")?;
            return Ok(ManagedPluginTarget {
                workspace_path,
                agent_session_id: Some(attached.agent_session_id),
            });
        }
        Ok(ManagedPluginTarget {
            workspace_path: persisted_workspace(app_session_id)?,
            agent_session_id: None,
        })
    }

    async fn exact_managed_plugin_discovery(
        &self,
        app_session_id: &str,
        attached: &AttachedManagedSession,
    ) -> Result<BridgeStatus, String> {
        let generation = managed_plugin_bridge::state_generation(app_session_id)?;
        if !attached
            .acp
            .managed_plugin_session_matches(app_session_id, &generation)
        {
            return Err("Managed MCP bridge is not bound to this agent process generation".into());
        }
        let server_name = managed_plugin_bridge::server_name_for_session(app_session_id)?;
        let mut last_error =
            "Managed MCP was not discovered by the exact agent session".to_string();
        for _ in 0..12 {
            match attached
                .acp
                .managed_mcp_list_for(&attached.agent_session_id)
                .await
                .and_then(|value| discovered_bridge(value, &server_name))
            {
                Ok(Some(tools)) => {
                    let status = managed_plugin_bridge::set_status(
                        app_session_id,
                        &generation,
                        BridgeLifecycleStatus::Applied,
                        Some(&attached.agent_session_id),
                        tools,
                        None,
                    )?;
                    managed_plugin_bridge::cleanup_old_generations(app_session_id, &generation);
                    return Ok(status);
                }
                Ok(None) => {
                    last_error = "Managed MCP is still initializing".into();
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    last_error = error;
                    break;
                }
            }
        }
        let _ = managed_plugin_bridge::set_status(
            app_session_id,
            &generation,
            BridgeLifecycleStatus::Failed,
            Some(&attached.agent_session_id),
            Vec::new(),
            Some(last_error.clone()),
        );
        Err(last_error)
    }

    pub(super) async fn hot_apply_managed_plugin(
        &self,
        app_session_id: &str,
        attached: &AttachedManagedSession,
    ) -> Result<BridgeStatus, String> {
        let mut applied_generation = managed_plugin_bridge::state_generation(app_session_id)?;
        let result = async {
            let bound = managed_plugin_bridge::bind_agent_session(
                app_session_id,
                &attached.agent_session_id,
            )?;
            applied_generation = bound
                .generation
                .ok_or("Managed MCP generation is unavailable")?;
            attached.acp.bind_managed_plugin_session(app_session_id);
            let cwd = attached.project_path.clone();
            let servers = tauri::async_runtime::spawn_blocking(move || {
                crate::extensions::build_session_mcp_servers(Some(cwd.as_str()))
            })
            .await
            .map_err(|_| "Could not build the existing MCP server set")?;
            attached
                .acp
                .update_mcp_servers(&attached.agent_session_id, servers)
                .await?;
            self.exact_managed_plugin_discovery(app_session_id, attached)
                .await
        }
        .await;
        if let Err(error) = result.as_ref() {
            let _ = managed_plugin_bridge::set_status(
                app_session_id,
                &applied_generation,
                BridgeLifecycleStatus::Failed,
                Some(&attached.agent_session_id),
                Vec::new(),
                Some(error.clone()),
            );
        }
        result
    }

    pub async fn apply_managed_plugin_bridge(
        &self,
        app_session_id: &str,
    ) -> Result<BridgeStatus, String> {
        let generation = managed_plugin_bridge::state_generation(app_session_id)?;
        let Some(attached) = self.attached_managed_session(app_session_id) else {
            return managed_plugin_bridge::set_status(
                app_session_id,
                &generation,
                BridgeLifecycleStatus::Waiting,
                None,
                Vec::new(),
                None,
            );
        };
        if attached.busy {
            self.pending_managed_plugin_apply
                .lock()
                .insert(app_session_id.to_string());
            return managed_plugin_bridge::set_status(
                app_session_id,
                &generation,
                BridgeLifecycleStatus::Waiting,
                Some(&attached.agent_session_id),
                Vec::new(),
                None,
            );
        }
        self.pending_managed_plugin_apply
            .lock()
            .remove(app_session_id);
        self.hot_apply_managed_plugin(app_session_id, &attached)
            .await
    }

    pub async fn flush_pending_managed_plugin_apply(&self, _app: &AppHandle, app_session_id: &str) {
        if !self
            .pending_managed_plugin_apply
            .lock()
            .remove(app_session_id)
        {
            return;
        }
        let Some(attached) = self.attached_managed_session(app_session_id) else {
            return;
        };
        if attached.busy {
            self.pending_managed_plugin_apply
                .lock()
                .insert(app_session_id.to_string());
            return;
        }
        let removing = managed_plugin_bridge::status(app_session_id)
            .ok()
            .is_some_and(|status| status.status == BridgeLifecycleStatus::Removing);
        let result = if removing {
            attached.acp.clear_managed_plugin_session(app_session_id);
            let cwd = attached.project_path.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                crate::extensions::build_session_mcp_servers(Some(cwd.as_str()))
            })
            .await
            {
                Ok(servers) => attached
                    .acp
                    .update_mcp_servers(&attached.agent_session_id, servers)
                    .await
                    .and_then(|_| managed_plugin_bridge::finish_remove(app_session_id).map(|_| ())),
                Err(_) => Err("Could not build the existing MCP server set".into()),
            }
        } else {
            self.hot_apply_managed_plugin(app_session_id, &attached)
                .await
                .map(|_| ())
        };
        if let Err(error) = result {
            tracing::warn!(session = %app_session_id, %error, "deferred managed MCP apply failed");
        }
    }

    pub async fn managed_plugin_after_connect(
        &self,
        app_session_id: &str,
        agent_session_id: &str,
        acp: Arc<AcpClient>,
        project_path: String,
    ) {
        let Ok(status) = managed_plugin_bridge::status(app_session_id) else {
            return;
        };
        if status.status == BridgeLifecycleStatus::Removing {
            self.pending_managed_plugin_apply
                .lock()
                .remove(app_session_id);
            let _ = managed_plugin_bridge::finish_remove(app_session_id);
            return;
        }
        if !status.configured {
            return;
        }
        self.pending_managed_plugin_apply
            .lock()
            .remove(app_session_id);
        let attached = AttachedManagedSession {
            acp,
            agent_session_id: agent_session_id.to_string(),
            project_path,
            busy: false,
        };
        if let Err(error) = self
            .hot_apply_managed_plugin(app_session_id, &attached)
            .await
        {
            tracing::warn!(session = %app_session_id, %error, "managed MCP reconnect apply failed");
        }
    }

    pub async fn retry_managed_plugin_bridge(
        &self,
        app_session_id: &str,
    ) -> Result<BridgeStatus, String> {
        let status = managed_plugin_bridge::status(app_session_id)?;
        if status.status == BridgeLifecycleStatus::Removing {
            let Some(attached) = self.attached_managed_session(app_session_id) else {
                return managed_plugin_bridge::finish_remove(app_session_id);
            };
            if attached.busy {
                self.pending_managed_plugin_apply
                    .lock()
                    .insert(app_session_id.to_string());
                return Ok(status);
            }
            attached.acp.clear_managed_plugin_session(app_session_id);
            let cwd = attached.project_path.clone();
            let servers = tauri::async_runtime::spawn_blocking(move || {
                crate::extensions::build_session_mcp_servers(Some(cwd.as_str()))
            })
            .await
            .map_err(|_| "Could not build the existing MCP server set")?;
            attached
                .acp
                .update_mcp_servers(&attached.agent_session_id, servers)
                .await?;
            return managed_plugin_bridge::finish_remove(app_session_id);
        }
        self.apply_managed_plugin_bridge(app_session_id).await
    }

    pub async fn verify_managed_plugin_call(
        &self,
        app_session_id: &str,
        expected_revision: u64,
        tool: &str,
        arguments: Value,
        consent: bool,
    ) -> Result<BridgeStatus, String> {
        if !consent {
            return Err("Explicit consent is required for a real MCP verification call".into());
        }
        if !arguments.is_object() || arguments.to_string().len() > 64 * 1024 {
            return Err("Verification arguments must be a bounded JSON object".into());
        }
        let status = managed_plugin_bridge::status(app_session_id)?;
        if status.revision != Some(expected_revision) {
            return Err("Managed MCP binding revision changed; refresh before retrying".into());
        }
        if !status.tools.iter().any(|candidate| candidate.name == tool) {
            return Err("Select a tool discovered in the exact agent session".into());
        }
        let attached = self
            .attached_managed_session(app_session_id)
            .ok_or("Selected agent session is not connected")?;
        if attached.busy {
            return Err("Wait for the selected agent turn to finish before verification".into());
        }
        let server_name = status
            .server_name
            .ok_or("Managed MCP server is unavailable")?;
        let response = unwrap_extension_result(
            attached
                .acp
                .managed_mcp_call_for(&attached.agent_session_id, &server_name, tool, arguments)
                .await?,
        )?;
        if response.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err("The selected MCP verification call returned an error".into());
        }
        let generation = status
            .generation
            .ok_or("Managed MCP generation is unavailable")?;
        managed_plugin_bridge::set_status(
            app_session_id,
            &generation,
            BridgeLifecycleStatus::Ready,
            Some(&attached.agent_session_id),
            status.tools,
            None,
        )
    }

    pub async fn remove_managed_plugin_bridge(
        &self,
        app_session_id: &str,
        expected_revision: u64,
    ) -> Result<BridgeStatus, String> {
        managed_plugin_bridge::begin_remove(app_session_id, expected_revision)?;
        let Some(attached) = self.attached_managed_session(app_session_id) else {
            self.pending_managed_plugin_apply
                .lock()
                .remove(app_session_id);
            return managed_plugin_bridge::finish_remove(app_session_id);
        };
        if attached.busy {
            self.pending_managed_plugin_apply
                .lock()
                .insert(app_session_id.to_string());
            return managed_plugin_bridge::status(app_session_id);
        }
        attached.acp.clear_managed_plugin_session(app_session_id);
        let cwd = attached.project_path.clone();
        let servers = tauri::async_runtime::spawn_blocking(move || {
            crate::extensions::build_session_mcp_servers(Some(cwd.as_str()))
        })
        .await
        .map_err(|_| "Could not build the existing MCP server set")?;
        attached
            .acp
            .update_mcp_servers(&attached.agent_session_id, servers)
            .await?;
        self.pending_managed_plugin_apply
            .lock()
            .remove(app_session_id);
        managed_plugin_bridge::finish_remove(app_session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_discovery_requires_session_state() {
        let server = "supercharge-plugin-api-aabb";
        assert!(discovered_bridge(
            serde_json::json!({"result":{"servers":[{"name":server}]}}),
            server,
        )
        .is_err());
    }

    #[test]
    fn exact_discovery_returns_only_ready_tool_summaries() {
        let server = "supercharge-plugin-api-aabb";
        let tools = discovered_bridge(
            serde_json::json!({"result":{"servers":[{
                "name":server,
                "session":{"enabled":true,"status":"ready","tools":[{
                    "name":"t_123","description":"Echo"
                }]}
            }]}}),
            server,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            tools,
            vec![BridgeToolSummary {
                name: "t_123".into(),
                description: Some("Echo".into())
            }]
        );
    }

    #[test]
    fn initializing_discovery_is_not_applied() {
        let server = "supercharge-plugin-api-aabb";
        assert_eq!(
            discovered_bridge(
                serde_json::json!({"result":{"servers":[{
                    "name":server,
                    "session":{"enabled":true,"status":"initializing","tools":[]}
                }]}}),
                server,
            )
            .unwrap(),
            None
        );
    }
}
