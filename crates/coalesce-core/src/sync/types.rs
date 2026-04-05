use serde::{Deserialize, Serialize};

/// Configuration for cloud/local sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub enabled: bool,
    pub method: SyncMethod,
    pub auto_sync: bool,
    pub sync_interval_secs: u64,
    pub device_id: String,
    pub last_sync: Option<String>,
}

/// How data is synchronized.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SyncMethod {
    #[serde(rename = "git")]
    Git { repo_url: String, branch: String },
    #[serde(rename = "webdav")]
    WebDav {
        url: String,
        username: String,
        password: String,
    },
    #[serde(rename = "local")]
    LocalDir { path: String },
}

/// Current sync status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub enabled: bool,
    pub method: String,
    pub last_sync: Option<String>,
    pub device_id: String,
    pub files_synced: usize,
    pub conflicts: Vec<SyncConflict>,
}

/// A file that has conflicting local and remote versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConflict {
    pub file: String,
    pub local_modified: String,
    pub remote_modified: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            method: SyncMethod::LocalDir {
                path: String::new(),
            },
            auto_sync: false,
            sync_interval_secs: 300,
            device_id: uuid::Uuid::new_v4().to_string(),
            last_sync: None,
        }
    }
}
