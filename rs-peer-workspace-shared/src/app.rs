use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEnvelope {
    pub message_id: Uuid,
    pub payload: AppPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppPayload {
    RpcRequest(RpcRequest),
    RpcResponse(RpcResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub request_id: Uuid,
    pub action: RpcAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RpcAction {
    RunCommand { command: String },
    ListRoots,
    ListDirectory { path: String },
    ReadFile { path: String },
    WriteFile { path: String, content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub request_id: Uuid,
    pub result: RpcResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RpcResult {
    CommandOutput { output: String },
    Roots { roots: Vec<String> },
    DirectoryEntries { path: String, entries: Vec<DirectoryEntry> },
    FileContent { path: String, content: String },
    WriteComplete { path: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}
