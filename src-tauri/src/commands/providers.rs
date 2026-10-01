/// Scan CC Switch Supercharge providers (read-only SQLite).
#[tauri::command]
pub async fn providers_cc_switch_scan(
) -> Result<crate::cc_switch_import::CcSwitchScanResult, String> {
    tauri::async_runtime::spawn_blocking(crate::cc_switch_import::scan_cc_switch_providers)
        .await
        .map_err(|e| e.to_string())
}

/// Import selected CC Switch Supercharge providers into App custom providers.
#[tauri::command]
pub async fn providers_cc_switch_import(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    body: crate::cc_switch_import::CcSwitchImportRequest,
) -> Result<crate::cc_switch_import::CcSwitchImportResult, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::cc_switch_import::import_cc_switch_providers(body)
    })
    .await
    .map_err(|e| e.to_string())??;
    if result.imported > 0 {
        mgr.recycle_all_agents(&app, "provider_route").await;
    }
    Ok(result)
}

#[tauri::command]
pub async fn providers_list() -> Result<crate::providers::ProvidersListResult, String> {
    // Blocking file I/O off the async runtime (migrations / repairs / list).
    tauri::async_runtime::spawn_blocking(|| {
        // Repairs are process-start migration work, not list work. Settings and
        // the composer can request this catalog independently; repeating all
        // config/secrets reads and writes made the Providers page feel blocked.
        static REPAIRS: std::sync::Once = std::sync::Once::new();
        REPAIRS.call_once(|| {
            let secrets = store::load_secrets();
            let _ = crate::providers::maybe_migrate_legacy_relay(
                secrets.relay_base_url.as_deref(),
                secrets.relay_api_key.as_deref(),
                secrets.default_model.as_deref(),
            );
            let _ = crate::providers::ensure_models_retry_cap();
            let _ = crate::providers::repair_custom_base_urls();
            let _ = crate::relay_stream_proxy::repair_sanitize_proxy_bases();
        });
        crate::providers::list_custom_providers()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Activate the catalog-managed default or an App-managed provider.
///
/// Recycles warm agents so the next send uses the updated config without a
/// full app restart. Command and DTO names stay stable for compatibility.
#[tauri::command]
pub async fn providers_activate(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    source: String,
    provider_id: Option<String>,
) -> Result<crate::providers::ProvidersListResult, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = crate::providers::activate_provider(&source, provider_id.as_deref())?;
        // Keep the composer preference on a real model id. The stable
        // `official` source value now means the catalog-managed default route.
        let mut settings = store::load_settings();
        let cur = settings.model_id.clone().unwrap_or_default();
        if result.active_source == "official" {
            let catalog_default = crate::models_catalog::list_available_models().default_model_id;
            if !catalog_default.is_empty()
                && (cur.is_empty() || crate::providers::is_custom_provider_id(&cur))
            {
                settings.model_id = Some(catalog_default);
                let _ = store::save_settings(&settings);
            }
        } else if result.active_source == "custom" {
            if let Some(upstream) = result
                .active_provider_id
                .as_ref()
                .and_then(|id| result.providers.iter().find(|provider| provider.id == *id))
                .map(|provider| provider.model.trim())
                .filter(|model| !model.is_empty())
            {
                settings.model_id = Some(upstream.to_string());
                let _ = store::save_settings(&settings);
            }
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Parked processes keep old provider config in memory — kill them.
    mgr.recycle_all_agents(&app, "provider_route").await;
    Ok(result)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn providers_upsert(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    id: String,
    model: String,
    base_url: String,
    name: Option<String>,
    api_key: Option<String>,
    api_backend: Option<String>,
    provider_mode: Option<String>,
    set_as_default: Option<bool>,
    create_only: Option<bool>,
    models: Option<Vec<crate::providers::ProviderModelEntry>>,
    efforts: Option<Vec<crate::providers::ProviderEffortEntry>>,
    context_window: Option<u64>,
    base_url_full_path: Option<bool>,
    append_prompt: Option<String>,
    supports_vision: Option<bool>,
    extra_headers: Option<Vec<crate::providers::ProviderHeaderEntry>>,
) -> Result<crate::providers::ProvidersListResult, String> {
    // Legacy provider modes are accepted on the wire but Supercharge resolves
    // every configured provider through the same generic config contract.
    let normalized_provider_mode =
        crate::providers::normalize_provider_mode(provider_mode.as_deref());
    let set_default_flag = set_as_default.unwrap_or(false);
    let mutated_id = id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result =
            crate::providers::upsert_custom_provider(crate::providers::UpsertProviderInput {
                id,
                model: model.clone(),
                base_url,
                name,
                api_key,
                api_backend,
                provider_mode: Some(normalized_provider_mode),
                set_as_default,
                create_only,
                models,
                efforts,
                context_window,
                base_url_full_path,
                append_prompt,
                supports_vision,
                extra_headers,
            })?;
        // Keep legacy secrets in sync for Doctor / account channel display.
        if let Some(p) = result
            .providers
            .iter()
            .find(|p| p.is_default)
            .or(result.providers.first())
        {
            let mut secrets = store::load_secrets();
            secrets.relay_base_url = Some(p.base_url.clone());
            secrets.default_model = result.default_model.clone();
            // Do not copy api_key into secrets (stays only in config.toml).
            let _ = store::save_secrets(&secrets);
            if set_as_default.unwrap_or(false) {
                let mut settings = store::load_settings();
                // Composer shows upstream request model, not the route slug.
                let upstream = p.model.trim();
                if !upstream.is_empty() {
                    settings.model_id = Some(upstream.to_string());
                    let _ = store::save_settings(&settings);
                }
            }
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Apply active-route / active-provider edits without requiring app restart.
    // Recycle rather than park so no process reopens with stale provider config.
    if crate::providers::provider_mutation_needs_agent_reload(
        set_default_flag,
        &mutated_id,
        &result,
    ) {
        mgr.recycle_all_agents(&app, "provider_route").await;
    }
    Ok(result)
}

#[tauri::command]
pub async fn providers_remove(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    id: String,
) -> Result<crate::providers::ProvidersListResult, String> {
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::providers::remove_custom_provider(&id))
            .await
            .map_err(|e| e.to_string())??;
    // Removing a provider (especially the active one) must not leave warm agents
    // on a deleted route id.
    mgr.recycle_all_agents(&app, "provider_route").await;
    Ok(result)
}

#[tauri::command]
pub async fn providers_set_default(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    model_id: String,
) -> Result<crate::providers::ProvidersListResult, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        let id = model_id.trim().to_string();
        if id.is_empty() {
            return Err("modelId is required".into());
        }
        let list = crate::providers::list_custom_providers()?;
        let result = if list.providers.iter().any(|provider| provider.id == id) {
            crate::providers::activate_provider("custom", Some(&id))?
        } else {
            crate::providers::set_default_model_id(&id)?
        };
        let mut settings = store::load_settings();
        let selected_model = result
            .active_provider_id
            .as_ref()
            .and_then(|provider_id| {
                result
                    .providers
                    .iter()
                    .find(|provider| provider.id == *provider_id)
            })
            .map(|provider| provider.model.trim())
            .filter(|model| !model.is_empty())
            .unwrap_or(id.as_str());
        settings.model_id = Some(selected_model.to_string());
        let _ = store::save_settings(&settings);
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| e.to_string())??;

    mgr.recycle_all_agents(&app, "provider_route").await;
    Ok(result)
}

#[tauri::command]
pub async fn providers_ping(
    base_url: Option<String>,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<crate::providers::ProviderPingResult, String> {
    crate::providers::ping_provider(base_url, api_key, provider_id).await
}

#[tauri::command]
pub async fn providers_list_models(
    base_url: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<crate::providers::RemoteModelsResult, String> {
    crate::providers::list_remote_models(base_url, api_key, provider_id).await
}

/// Test whether a specific model id is usable on a custom provider.
/// Sends one tiny non-streaming inference request; success = HTTP 2xx.
#[tauri::command]
pub async fn providers_test_model(
    base_url: Option<String>,
    api_key: Option<String>,
    provider_id: Option<String>,
    model: String,
    api_backend: Option<String>,
    base_url_full_path: Option<bool>,
) -> Result<crate::providers::ProviderTestResult, String> {
    crate::providers::test_model_connection(
        base_url,
        api_key,
        provider_id,
        model,
        api_backend,
        base_url_full_path,
    )
    .await
}

/// Probe provider account balance (Phase 1: DeepSeek `GET /user/balance` only).
#[tauri::command]
pub async fn providers_balance(
    base_url: Option<String>,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<crate::providers::ProviderBalanceResult, String> {
    crate::providers::query_provider_balance(base_url, api_key, provider_id).await
}

// ── Model auxiliary routing (`[models]` side-task slots) ─────────────────────

#[tauri::command]
pub async fn models_aux_get() -> Result<crate::models_aux::ModelsAuxState, String> {
    tauri::async_runtime::spawn_blocking(crate::models_aux::get_state)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn models_aux_set(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
    image_description: Option<String>,
    web_search: Option<String>,
    session_summary: Option<String>,
    prompt_suggestion: Option<String>,
) -> Result<crate::models_aux::ModelsAuxState, String> {
    let input = crate::models_aux::ModelsAuxSetInput {
        image_description,
        web_search,
        session_summary,
        prompt_suggestion,
    };
    let result = tauri::async_runtime::spawn_blocking(move || crate::models_aux::set_slots(input))
        .await
        .map_err(|e| e.to_string())??;
    mgr.recycle_all_agents(&app, "models_aux").await;
    Ok(result)
}

#[tauri::command]
pub async fn models_aux_apply_save_grok(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
) -> Result<crate::models_aux::ModelsAuxState, String> {
    let result = tauri::async_runtime::spawn_blocking(crate::models_aux::apply_save_grok)
        .await
        .map_err(|e| e.to_string())??;
    mgr.recycle_all_agents(&app, "models_aux").await;
    Ok(result)
}

#[tauri::command]
pub async fn models_aux_reset_defaults(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<SessionManager>>,
) -> Result<crate::models_aux::ModelsAuxState, String> {
    let result = tauri::async_runtime::spawn_blocking(crate::models_aux::reset_defaults)
        .await
        .map_err(|e| e.to_string())??;
    mgr.recycle_all_agents(&app, "models_aux").await;
    Ok(result)
}

/// Independent side-channel: `supercharge -p -m <aux>` under agent-home (not live session model).
#[tauri::command]
pub async fn models_aux_headless(
    model_id: String,
    prompt: String,
    max_turns: Option<u32>,
) -> Result<String, String> {
    let turns = max_turns.unwrap_or(8);
    tauri::async_runtime::spawn_blocking(move || {
        crate::models_aux::run_aux_headless(
            &model_id,
            &prompt,
            turns,
            std::time::Duration::from_secs(180),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Host web search via aux model headless (ignores live ACP main model).
#[tauri::command]
pub async fn models_aux_web_search(query: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::models_aux::headless_web_search(&query))
        .await
        .map_err(|e| e.to_string())?
}

// ── Official aux side-channel (isolated GROK_HOME + grok -p) ─────────────────

#[tauri::command]
pub async fn official_aux_status() -> Result<crate::official_aux::OfficialAuxStatus, String> {
    Ok(crate::official_aux::status())
}

#[tauri::command]
pub async fn official_aux_ensure_home() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::official_aux::ensure_official_aux_home().map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_dispatch(
    tool: String,
    args: serde_json::Value,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::official_aux::dispatch_tool(&tool, &args))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_web_search(query: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::official_aux::web_search(&query))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_x_keyword_search(
    query: String,
    limit: Option<u32>,
    min_faves: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::official_aux::x_keyword_search(&query, limit, min_faves)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_x_semantic_search(
    query: String,
    limit: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::official_aux::x_semantic_search(&query, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_x_user_search(
    query: String,
    count: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::official_aux::x_user_search(&query, count))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_x_thread_fetch(post_id_or_url: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::official_aux::x_thread_fetch(&post_id_or_url)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn official_aux_vision_describe(
    paths: Vec<String>,
    question: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::official_aux::vision_describe(&paths, question.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Editors ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn editors_list() -> Result<crate::editors::EditorsListResult, String> {
    Ok(crate::editors::list_editors_with_icons())
}

#[tauri::command]
pub async fn open_in_editor(
    path: String,
    line: Option<u32>,
    editor: Option<String>,
) -> Result<(), String> {
    let settings = store::load_settings_async().await;
    let target = editor
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| settings.default_open_target.clone());
    crate::editors::open_in_editor(&path, line, Some(target.as_str()))
}

#[cfg(test)]
mod project_inspect_tests {
    use super::build_project_inspect_summary;

    #[test]
    fn summary_strips_mcp_env_and_skill_descriptions() {
        let raw = serde_json::json!({
            "grokVersion": "0.2.0",
            "projectRoot": "/tmp/p/",
            "projectTrusted": true,
            "skills": [{
                "name": "help",
                "description": "secret sk-abcdefghijklmnopqrstuvwxyz",
                "source": { "type": "user" },
                "userInvocable": true
            }],
            "mcpServers": [{
                "name": "ctx",
                "transport": "stdio",
                "target": "/bin/npx",
                "env": { "API_KEY": "sk-secretsecretsecret" }
            }],
            "plugins": [{ "name": "p1", "scope": "user", "enabled": true }],
            "agents": [{ "name": "explore", "source": { "type": "builtin" } }],
            "projectInstructions": [{ "path": "/tmp/p/AGENTS.md", "scope": "project" }],
            "hooks": [{
                "event": "stop",
                "hookType": "file",
                "target": "/tmp/p/.grok/hooks/stop.json",
                "source": { "type": "project" }
            }],
            "permissions": { "loaded": 0, "sources": [], "managedSettingsActive": false }
        });
        let out =
            build_project_inspect_summary(Some(&raw), Some("/tmp/p"), None, vec!["grok-4".into()]);
        let s = out.to_string();
        assert!(s.contains("\"help\""));
        assert!(s.contains("\"ctx\""));
        assert!(s.contains("AGENTS.md"));
        assert!(!s.contains("sk-secret"));
        assert!(!s.contains("API_KEY"));
        assert!(!s.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert_eq!(out["skills"]["total"], 1);
        assert_eq!(out["skills"]["names"][0], "help");
        assert_eq!(out["mcp"][0]["name"], "ctx");
        assert!(out["mcp"][0].get("env").is_none());
        assert_eq!(out["hooksCount"], 1);
        assert_eq!(out["hooks"][0]["event"], "stop");
        assert_eq!(out["hooks"][0]["source"], "project");
        assert_eq!(out["agents"][0]["name"], "explore");
        assert!(out["modelsHints"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("grok-4")));
    }

    #[test]
    fn summary_handles_missing_inspect() {
        let out = build_project_inspect_summary(
            None,
            Some("/tmp/p"),
            Some("Supercharge CLI not found".into()),
            vec![],
        );
        assert_eq!(out["skills"]["total"], 0);
        assert_eq!(out["error"], "Supercharge CLI not found");
    }
}
