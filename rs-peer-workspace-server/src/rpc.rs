use std::path::{Path, PathBuf};

use tokio::fs;

use crate::protocol::{DirectoryEntry, RpcAction, RpcRequest, RpcResponse, RpcResult};

pub async fn handle_rpc(request: RpcRequest) -> RpcResponse {
    let result = match request.action {
        RpcAction::RunCommand { command } => RpcResult::CommandOutput {
            output: execute_command(command).await,
        },
        RpcAction::ListRoots => match list_roots() {
            Ok(roots) => RpcResult::Roots { roots },
            Err(err) => RpcResult::Error {
                message: err.to_string(),
            },
        },
        RpcAction::ListDirectory { path } => match list_directory(&path).await {
            Ok(entries) => RpcResult::DirectoryEntries { path, entries },
            Err(err) => RpcResult::Error {
                message: err.to_string(),
            },
        },
        RpcAction::ReadFile { path } => match fs::read_to_string(&path).await {
            Ok(content) => RpcResult::FileContent { path, content },
            Err(err) => RpcResult::Error {
                message: err.to_string(),
            },
        },
        RpcAction::WriteFile { path, content } => {
            let result = async {
                if let Some(parent) = Path::new(&path).parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::write(&path, content).await?;
                anyhow::Ok(())
            }
            .await;

            match result {
                Ok(()) => RpcResult::WriteComplete { path },
                Err(err) => RpcResult::Error {
                    message: err.to_string(),
                },
            }
        }
    };

    RpcResponse {
        request_id: request.request_id,
        result,
    }
}

fn list_roots() -> anyhow::Result<Vec<String>> {
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if PathBuf::from(&drive).exists() {
                roots.push(drive);
            }
        }
        return Ok(roots);
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(vec!["/".to_string()])
    }
}

async fn list_directory(path: &str) -> anyhow::Result<Vec<DirectoryEntry>> {
    let mut dir = fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        let entry_path = entry.path();
        let metadata = entry.metadata().await?;
        entries.push(DirectoryEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry_path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        a.is_dir
            .cmp(&b.is_dir)
            .reverse()
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

async fn execute_command(command: String) -> String {
    #[cfg(target_os = "windows")]
    let output_result = tokio::process::Command::new("powershell")
        .arg("-Command")
        .arg(command)
        .output()
        .await;

    #[cfg(not(target_os = "windows"))]
    let output_result = tokio::process::Command::new("sh")
        .arg("-lc")
        .arg(command)
        .output()
        .await;

    match output_result {
        Ok(output) => {
            let mut combined = String::new();
            if !output.stdout.is_empty() {
                combined.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            if combined.is_empty() {
                "<no output>".to_string()
            } else {
                combined
            }
        }
        Err(err) => format!("command execution failed: {err}"),
    }
}
