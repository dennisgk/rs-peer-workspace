use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectFile {
    pub connections: Vec<ProjectConnection>,
    pub folders: Vec<ProjectFolder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConnection {
    pub name: String,
    pub proxy_addr: String,
    pub proxy_password: String,
    pub server_name: String,
    pub server_password: String,
    pub prefer_p2p: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFolder {
    pub name: String,
    pub source: FolderSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FolderSource {
    Local { path: String },
    Remote { connection_name: String, path: String },
}

#[derive(Debug, Clone)]
pub enum EditorSource {
    Local,
    Remote { connection_name: String },
}

#[derive(Debug, Clone)]
pub struct EditorTab {
    pub title: String,
    pub path: String,
    pub source: EditorSource,
    pub content: String,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct TerminalTab {
    pub id: Uuid,
    pub connection_name: String,
    pub title: String,
    pub input: String,
    pub output: String,
}

pub fn default_connection_form_addr() -> String {
    "ws://127.0.0.1:9000/ws".to_string()
}

pub fn display_name_for_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

pub fn is_text_file(path: &str) -> bool {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "txt" | "py" | "m"),
        None => false,
    }
}
