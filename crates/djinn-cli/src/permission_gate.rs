use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use djinn_agent::{PermissionDecision, PermissionGate, PermissionRequest};
use serde_json::Value;

#[derive(Debug, Default)]
pub(crate) struct TerminalPermissionGate {
    session_scopes: Mutex<Vec<TerminalApprovalScope>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalApprovalScope {
    action: String,
    workspace: String,
    resources: HashSet<String>,
}

impl TerminalPermissionGate {
    pub(crate) fn new() -> Self {
        Self {
            session_scopes: Mutex::new(Vec::new()),
        }
    }

    fn cached_decision(&self, request: &PermissionRequest) -> Option<PermissionDecision> {
        let request_resources = approval_resources_from_metadata(&request.metadata);
        if request_resources.is_empty() {
            return None;
        }
        let workspace = request
            .metadata
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scopes = self.session_scopes.lock().ok()?;
        let mut approved = Vec::new();
        for resource in &request_resources {
            let covered = scopes.iter().any(|scope| {
                scope.action == request.action
                    && scope.workspace == workspace
                    && scope.resources.contains(resource)
            });
            if !covered {
                return None;
            }
            approved.push(resource.clone());
        }
        if request
            .metadata
            .get("preview")
            .and_then(Value::as_array)
            .is_some()
        {
            Some(PermissionDecision::AllowPaths { paths: approved })
        } else {
            Some(PermissionDecision::AllowResources {
                resources: approved,
            })
        }
    }

    fn remember_resources_for_session(&self, request: &PermissionRequest, resources: Vec<String>) {
        let resources = resources
            .into_iter()
            .map(|resource| resource.trim().to_string())
            .filter(|resource| !resource.is_empty())
            .collect::<HashSet<_>>();
        if resources.is_empty() {
            return;
        }
        let workspace = request
            .metadata
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Ok(mut scopes) = self.session_scopes.lock() else {
            return;
        };
        if let Some(existing) = scopes
            .iter_mut()
            .find(|scope| scope.action == request.action && scope.workspace == workspace)
        {
            existing.resources.extend(resources);
        } else {
            scopes.push(TerminalApprovalScope {
                action: request.action.clone(),
                workspace,
                resources,
            });
        }
    }

    fn report_permission_blocked(&self, _request: &PermissionRequest) {}

    fn report_permission_resolved(&self) {}
}

#[async_trait]
impl PermissionGate for TerminalPermissionGate {
    async fn approve(&self, request: PermissionRequest) -> Result<PermissionDecision> {
        if let Some(decision) = self.cached_decision(&request) {
            return Ok(decision);
        }
        self.report_permission_blocked(&request);
        if request
            .metadata
            .get("preview")
            .and_then(Value::as_array)
            .is_some()
            && io::stdin().is_terminal()
            && io::stdout().is_terminal()
        {
            let decision = match djinn_tui::run_approval_dialog(request.metadata.clone())? {
                djinn_tui::ApprovalDecision::ApproveAll => PermissionDecision::Allow,
                djinn_tui::ApprovalDecision::ApprovePaths(paths) => {
                    PermissionDecision::AllowPaths { paths }
                }
                djinn_tui::ApprovalDecision::ApproveAllForSession(paths)
                | djinn_tui::ApprovalDecision::ApprovePathsForSession(paths) => {
                    self.remember_resources_for_session(&request, paths.clone());
                    PermissionDecision::AllowPaths { paths }
                }
                djinn_tui::ApprovalDecision::Deny => PermissionDecision::Deny,
            };
            self.report_permission_resolved();
            return Ok(decision);
        }
        eprintln!("\nPermission approval required: {}", request.description);
        eprint!("{}", format_permission_preview(&request.metadata)?);
        eprint!("Approve this request? [y]es once, [s]ession, [N]o: ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        let decision = if answer == "y" || answer == "yes" {
            PermissionDecision::Allow
        } else if answer == "s" || answer == "session" {
            let resources = approval_resources_from_metadata(&request.metadata);
            self.remember_resources_for_session(&request, resources.clone());
            if request
                .metadata
                .get("preview")
                .and_then(Value::as_array)
                .is_some()
            {
                PermissionDecision::AllowPaths { paths: resources }
            } else {
                PermissionDecision::AllowResources { resources }
            }
        } else {
            PermissionDecision::Deny
        };
        self.report_permission_resolved();
        Ok(decision)
    }
}

fn approval_resources_from_metadata(metadata: &Value) -> Vec<String> {
    let mut resources = Vec::new();
    if let Some(preview) = metadata.get("preview").and_then(Value::as_array) {
        for item in preview {
            if let Some(path) = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                push_unique_string(&mut resources, path);
            }
            if let Some(path) = item
                .get("new_path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                push_unique_string(&mut resources, path);
            }
        }
    }
    if let Some(values) = metadata.get("resources").and_then(Value::as_array) {
        for value in values {
            if let Some(resource) = value
                .as_str()
                .filter(|resource| !resource.trim().is_empty())
            {
                push_unique_string(&mut resources, resource);
            }
        }
    }
    if let Some(resource) = metadata
        .get("resource")
        .and_then(Value::as_str)
        .filter(|resource| !resource.trim().is_empty())
    {
        push_unique_string(&mut resources, resource);
    }
    resources
}

fn format_permission_preview(metadata: &Value) -> Result<String> {
    let Some(preview) = metadata.get("preview").and_then(Value::as_array) else {
        return Ok(format!("{}\n", serde_json::to_string_pretty(metadata)?));
    };
    let mut output = String::new();
    for item in preview {
        let operation = item["operation"].as_str().unwrap_or("operation");
        let path = item["relative_path"]
            .as_str()
            .or_else(|| item["path"].as_str())
            .unwrap_or("<unknown>");
        let added = item["lines_added"].as_u64().unwrap_or_default();
        let removed = item["lines_removed"].as_u64().unwrap_or_default();
        output.push_str(&format!("- {operation} {path} (+{added}/-{removed})\n"));
        if let Some(new_path) = item["relative_new_path"]
            .as_str()
            .or_else(|| item["new_path"].as_str())
        {
            output.push_str(&format!("  -> {new_path}\n"));
        }
        if let Some(hunks) = item["hunks"].as_array() {
            for (index, hunk) in hunks.iter().enumerate() {
                output.push_str(&format!("  @@ hunk {}\n", index + 1));
                if let Some(lines) = hunk["lines"].as_array() {
                    for line in lines {
                        let kind = line["kind"].as_str().unwrap_or("context");
                        let content = line["content"].as_str().unwrap_or_default();
                        let prefix = match kind {
                            "add" => '+',
                            "remove" => '-',
                            _ => ' ',
                        };
                        output.push_str(&format!("  {prefix} {content}\n"));
                    }
                }
            }
        }
    }
    Ok(output)
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if values.iter().any(|existing| existing == value) {
        return;
    }
    values.push(value.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_permission_preview_renders_full_hunks() {
        let rendered = format_permission_preview(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": [
                        {
                            "lines": [
                                {"kind": "context", "content": "fn answer() -> i32 {"},
                                {"kind": "remove", "content": "    41"},
                                {"kind": "add", "content": "    42"},
                                {"kind": "context", "content": "}"}
                            ]
                        }
                    ]
                }
            ]
        }))
        .unwrap();

        assert!(rendered.contains("- update src/lib.rs (+1/-1)"));
        assert!(rendered.contains("  @@ hunk 1"));
        assert!(rendered.contains("    fn answer() -> i32 {"));
        assert!(rendered.contains("  -     41"));
        assert!(rendered.contains("  +     42"));
    }

    #[test]
    fn terminal_permission_gate_reuses_session_path_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "apply_patch".to_string(),
            description: "patch".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "preview": [
                    {"path": "/tmp/work/a.txt", "relative_path": "a.txt"},
                    {"path": "/tmp/work/b.txt", "relative_path": "b.txt"}
                ]
            }),
        };

        assert!(gate.cached_decision(&request).is_none());
        gate.remember_resources_for_session(
            &request,
            vec!["/tmp/work/a.txt".to_string(), "/tmp/work/b.txt".to_string()],
        );

        assert_eq!(
            gate.cached_decision(&request),
            Some(PermissionDecision::AllowPaths {
                paths: vec!["/tmp/work/a.txt".to_string(), "/tmp/work/b.txt".to_string()]
            })
        );
    }

    #[test]
    fn terminal_permission_gate_does_not_reuse_partial_or_cross_action_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "apply_patch".to_string(),
            description: "patch".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "preview": [
                    {"path": "/tmp/work/a.txt", "relative_path": "a.txt"},
                    {"path": "/tmp/work/b.txt", "relative_path": "b.txt"}
                ]
            }),
        };
        let other_action = PermissionRequest {
            action: "write".to_string(),
            ..request.clone()
        };

        gate.remember_resources_for_session(&request, vec!["/tmp/work/a.txt".to_string()]);

        assert!(gate.cached_decision(&request).is_none());
        assert!(gate.cached_decision(&other_action).is_none());
    }

    #[test]
    fn terminal_permission_gate_reuses_session_resource_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "shell".to_string(),
            description: "shell".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "kind": "shell",
                "resource": "printf hello",
                "resources": ["printf hello"]
            }),
        };

        assert!(gate.cached_decision(&request).is_none());
        gate.remember_resources_for_session(&request, vec!["printf hello".to_string()]);

        assert_eq!(
            gate.cached_decision(&request),
            Some(PermissionDecision::AllowResources {
                resources: vec!["printf hello".to_string()]
            })
        );
    }
}
