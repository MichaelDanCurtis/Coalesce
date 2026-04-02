use serde_json::Value;

const PROXY_BASE: &str = "http://127.0.0.1:8402";

async fn proxy_get(path: &str) -> Result<Value, String> {
    let url = format!("{}{}", PROXY_BASE, path);
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    resp.json::<Value>()
        .await
        .map_err(|e| format!("Parse failed: {}", e))
}

async fn proxy_post(path: &str, body: Value) -> Result<Value, String> {
    let url = format!("{}{}", PROXY_BASE, path);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    resp.json::<Value>()
        .await
        .map_err(|e| format!("Parse failed: {}", e))
}

#[tauri::command]
pub async fn get_providers() -> Result<Value, String> {
    proxy_get("/api/v1/providers").await
}

#[tauri::command]
pub async fn get_stats() -> Result<Value, String> {
    proxy_get("/api/v1/stats/summary").await
}

#[tauri::command]
pub async fn get_models() -> Result<Value, String> {
    proxy_get("/v1/models").await
}

#[tauri::command]
pub async fn get_health() -> Result<Value, String> {
    proxy_get("/health").await
}

#[tauri::command]
pub async fn routing_playground(prompt: String) -> Result<Value, String> {
    proxy_post(
        "/api/v1/routing/playground",
        serde_json::json!({ "prompt": prompt }),
    )
    .await
}

#[tauri::command]
pub async fn get_quotas() -> Result<Value, String> {
    proxy_get("/api/v1/providers/quotas").await
}

#[tauri::command]
pub async fn get_profiles() -> Result<Value, String> {
    proxy_get("/api/v1/routing/profiles").await
}

#[tauri::command]
pub async fn get_timeline(limit: Option<u32>, offset: Option<u32>) -> Result<Value, String> {
    let limit = limit.unwrap_or(20);
    let offset = offset.unwrap_or(0);
    proxy_get(&format!("/api/v1/stats/timeline?limit={}&offset={}", limit, offset)).await
}

#[tauri::command]
pub async fn get_costs(days: Option<u32>) -> Result<Value, String> {
    let days = days.unwrap_or(30);
    proxy_get(&format!("/api/v1/stats/costs?days={}", days)).await
}
