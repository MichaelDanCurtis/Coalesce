use serde::Deserialize;
use tracing::info;

const PROXY_BASE: &str = "http://127.0.0.1:8402";

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum DeepLinkAction {
    #[serde(rename = "add-provider")]
    AddProvider {
        name: String,
        api_key: Option<String>,
        base_url: Option<String>,
    },
    #[serde(rename = "add-mcp")]
    AddMcp {
        name: String,
        transport: String,
        command: Option<String>,
        url: Option<String>,
        args: Option<Vec<String>>,
    },
    #[serde(rename = "import-profile")]
    ImportProfile {
        url: String,
    },
}

/// Parse a coalesce:// URL into an action.
/// Format: `coalesce://import/<base64url-encoded-JSON>`
pub fn parse_deep_link(raw: &str) -> Option<DeepLinkAction> {
    let url = raw.strip_prefix("coalesce://")?;

    if let Some(encoded) = url.strip_prefix("import/") {
        use base64::Engine;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()?;
        let action: DeepLinkAction = serde_json::from_slice(&decoded).ok()?;
        return Some(action);
    }

    // Query string format — log and return None for now
    info!("Deep link received (unhandled format): {}", raw);
    None
}

/// Execute a deep link action by calling the proxy API.
pub async fn execute_action(action: DeepLinkAction) -> Result<String, String> {
    let client = reqwest::Client::new();

    match action {
        DeepLinkAction::AddProvider { name, api_key, base_url } => {
            info!("Deep link: add-provider {}", name);
            Ok(format!(
                "Provider {} queued for setup (api_key: {}, base_url: {})",
                name,
                api_key.is_some(),
                base_url.unwrap_or_default()
            ))
        }
        DeepLinkAction::AddMcp {
            name,
            transport,
            command,
            url,
            args,
        } => {
            let body = serde_json::json!({
                "name": name,
                "transport": transport,
                "command": command,
                "url": url,
                "args": args,
            });
            client
                .post(format!("{}/api/v1/mcp/servers", PROXY_BASE))
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("Added MCP server: {}", name))
        }
        DeepLinkAction::ImportProfile { url: profile_url } => {
            let config_text = client
                .get(&profile_url)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .text()
                .await
                .map_err(|e| e.to_string())?;
            client
                .post(format!("{}/api/v1/profiles/import", PROXY_BASE))
                .json(&serde_json::json!({"config": config_text}))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            Ok("Profile imported".into())
        }
    }
}
