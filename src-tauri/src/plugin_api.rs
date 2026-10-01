use std::{collections::BTreeSet, sync::Arc, time::Duration};

use parking_lot::Mutex;
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_REPLY_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub struct PluginApiConnection {
    current: Mutex<Option<Arc<AuthenticatedService>>>,
}

struct AuthenticatedService {
    base_url: Url,
    token: String,
    client: Client,
    revision: String,
    capabilities: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connected: bool,
    pub endpoint: Option<String>,
    pub connection_id: Option<String>,
    pub capabilities: Option<Value>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginRequest {
    List,
    Validate { manifest: Value },
    Install { source: Value, idempotency_key: String },
    Action { plugin_id: String, action: Value, idempotency_key: String },
    Operation { operation_id: String },
}

fn endpoint(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| "Invalid Plugin API endpoint")?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1" | "localhost"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Use a HTTPS service origin or local loopback HTTP, without credentials or a path".into());
    }
    Ok(url)
}

fn safe_id(id: &str) -> Result<&str, String> {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        return Err("Invalid Plugin API identifier".into());
    }
    Ok(id)
}

fn mutation_key(key: &str) -> Result<&str, String> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| (33..=126).contains(&b)) {
        return Err("Invalid request identifier".into());
    }
    Ok(key)
}

impl AuthenticatedService {
    async fn request(&self, method: Method, path: &str, body: Option<Value>, key: Option<&str>) -> Result<Value, String> {
        let url = self.base_url.join(path).map_err(|_| "Invalid Plugin API route")?;
        if url.origin() != self.base_url.origin() {
            return Err("Plugin API route changed origin".into());
        }
        let mut request = self.client.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            if body.to_string().len() > 256 * 1024 { return Err("Plugin request exceeds limit".into()); }
            request = request.json(&body);
        }
        if let Some(key) = key { request = request.header("Idempotency-Key", mutation_key(key)?); }
        let mut response = request.send().await.map_err(|_| "Plugin API is unreachable; the request outcome may be unknown")?;
        let status = response.status();
        if response.content_length().is_some_and(|n| n > MAX_REPLY_BYTES as u64) {
            return Err("Plugin API response exceeds limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "Plugin API response was interrupted")? {
            if bytes.len() + chunk.len() > MAX_REPLY_BYTES { return Err("Plugin API response exceeds limit".into()); }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Plugin API returned an invalid response")?;
        if !status.is_success() {
            let code = value.pointer("/error/code").and_then(Value::as_str).filter(|c| c.len() <= 80 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')).unwrap_or("request_failed");
            return Err(format!("Plugin API {code} (HTTP {})", status.as_u16()));
        }
        Ok(value)
    }
}

impl PluginApiConnection {
    pub fn status(&self) -> ConnectionStatus {
        match self.current.lock().as_ref() {
            Some(service) => ConnectionStatus { connected: true, endpoint: Some(service.base_url.to_string()), connection_id: Some(service.revision.clone()), capabilities: Some(service.capabilities.clone()) },
            None => ConnectionStatus { connected: false, endpoint: None, connection_id: None, capabilities: None },
        }
    }

    pub async fn connect(&self, base_url: &str, token: String) -> Result<ConnectionStatus, String> {
        if token.len() < 32 || token.len() > 256 || token.chars().any(char::is_whitespace) { return Err("Plugin API key must contain 32–256 characters without whitespace".into()); }
        let mut service = AuthenticatedService {
            base_url: endpoint(base_url)?, token,
            client: Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().map_err(|_| "Could not initialize Plugin API client")?,
            revision: uuid::Uuid::new_v4().to_string(), capabilities: Value::Null,
        };
        let version = service.request(Method::GET, "/v1/version", None, None).await?;
        if version.get("apiVersion").and_then(Value::as_str) != Some("1") { return Err("Unsupported Plugin API version".into()); }
        service.capabilities = service.request(Method::GET, "/v1/capabilities", None, None).await?;
        if !service.capabilities.get("workspaceIds").is_some_and(Value::is_array) { return Err("Plugin API capabilities are invalid".into()); }
        *self.current.lock() = Some(Arc::new(service));
        Ok(self.status())
    }

    pub fn disconnect(&self) -> ConnectionStatus {
        self.current.lock().take();
        self.status()
    }

    pub async fn execute(&self, connection_id: &str, input: PluginRequest) -> Result<Value, String> {
        let service = self.current.lock().clone().ok_or("Connect to Plugin API first")?;
        if service.revision != connection_id { return Err("Plugin API connection changed; reload before retrying".into()); }
        match input {
            PluginRequest::List => service.request(Method::GET, "/v1/plugins", None, None).await,
            PluginRequest::Validate { manifest } => service.request(Method::POST, "/v1/plugins/validate", Some(json!({ "manifest": manifest })), None).await,
            PluginRequest::Install { source, idempotency_key } => {
                let kind = source.get("type").and_then(Value::as_str).ok_or("Plugin source is required")?;
                if !matches!(kind, "inline" | "archive") { return Err("Use an inline MCP definition or digest-verified HTTPS archive".into()); }
                service.request(Method::POST, "/v1/plugins/install", Some(json!({ "source": source })), Some(&idempotency_key)).await
            },
            PluginRequest::Action { plugin_id, action, idempotency_key } => {
                let name = action.get("action").and_then(Value::as_str).ok_or("Plugin action is required")?;
                let allowed = BTreeSet::from(["trust", "enable", "disable", "configure", "update", "rollback", "uninstall"]);
                if !allowed.contains(name) { return Err("Unsupported plugin action".into()); }
                if name == "update" && !matches!(action.pointer("/source/type").and_then(Value::as_str), Some("inline" | "archive")) { return Err("Use an inline definition or verified archive for updates".into()); }
                service.request(Method::POST, &format!("/v1/plugins/{}/actions", safe_id(&plugin_id)?), Some(action), Some(&idempotency_key)).await
            },
            PluginRequest::Operation { operation_id } => service.request(Method::GET, &format!("/v1/operations/{}", safe_id(&operation_id)?), None, None).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_credential_urls_and_nonlocal_plaintext() {
        for url in ["http://example.com", "https://user:pass@example.com", "https://example.com/v1", "http://127.0.0.1/?token=secret", "file:///etc/passwd"] {
            assert!(endpoint(url).is_err());
        }
        assert!(endpoint("http://127.0.0.1:4319").is_ok());
        assert!(endpoint("https://plugins.example.com").is_ok());
    }

    #[test]
    fn state_never_serializes_a_credential() {
        let state = PluginApiConnection::default();
        assert!(!state.status().connected);
        assert!(!serde_json::to_string(&state.status()).unwrap().contains("token"));
        assert!(!state.disconnect().connected);
    }

    #[test]
    fn identifiers_cannot_change_routes() {
        for value in ["../other", "a/b", "a?token=x", "", "a%2Fb"] { assert!(safe_id(value).is_err()); }
        assert!(safe_id("sample-native").is_ok());
        assert!(mutation_key("logical-request-1").is_ok());
        assert!(mutation_key("bad\nkey").is_err());
    }
}
