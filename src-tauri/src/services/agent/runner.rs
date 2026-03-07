use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex;

use crate::db::models::{AgentConfig, AgentHealthStatus, SandboxConfig};

use super::claude_code::ClaudeCodeRunner;
use super::codex::CodexRunner;
use super::crypto;
use super::worktree::WorktreeManager;

/// 从 Windows 注册表读取完整的系统 + 用户 PATH，解决 Tauri GUI
/// 进程启动时 PATH 不完整导致找不到 CLI 的问题。
#[cfg(target_os = "windows")]
fn get_registry_path() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut paths = Vec::new();

    if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")
    {
        if let Ok(val) = key.get_value::<String, _>("Path") {
            paths.push(val);
        }
    }

    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment") {
        if let Ok(val) = key.get_value::<String, _>("Path") {
            paths.push(val);
        }
    }

    if paths.is_empty() {
        None
    } else {
        Some(paths.join(";"))
    }
}

/// 合并当前进程 PATH 与注册表 PATH，返回完整的 PATH 字符串。
#[cfg(target_os = "windows")]
fn get_merged_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    match get_registry_path() {
        Some(reg) if !reg.is_empty() => {
            if current.is_empty() {
                reg
            } else {
                format!("{};{}", current, reg)
            }
        }
        _ => current,
    }
}

/// 在合并后的 PATH 目录中搜索可执行文件。
/// 返回找到的完整路径和文件类型（exe 或 cmd）。
#[cfg(target_os = "windows")]
enum ResolvedProgram {
    Exe(String),
    CmdScript { node_exe: String, script: String },
    NotFound,
}

#[cfg(target_os = "windows")]
fn resolve_in_path(program: &str) -> ResolvedProgram {
    let merged = get_merged_path();

    for dir in merged.split(';') {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        let base = Path::new(dir);

        let exe = base.join(format!("{}.exe", program));
        if exe.exists() {
            return ResolvedProgram::Exe(exe.to_string_lossy().to_string());
        }
        let com = base.join(format!("{}.com", program));
        if com.exists() {
            return ResolvedProgram::Exe(com.to_string_lossy().to_string());
        }

        let cmd_file = base.join(format!("{}.cmd", program));
        if cmd_file.exists() {
            if let Some((node, script)) = parse_npm_cmd_file(&cmd_file) {
                return ResolvedProgram::CmdScript {
                    node_exe: node,
                    script,
                };
            }
        }
    }

    ResolvedProgram::NotFound
}

/// 解析 npm 生成的 .cmd 包装脚本，提取底层的 node.exe 和 JS
/// 脚本路径，使得我们可以直接通过 node.exe 调用而不经过 cmd.exe，
/// 避免 Rust CVE-2024-24576 安全限制导致的参数校验失败。
///
/// npm .cmd 文件典型结构：
///   IF EXIST "%dp0%\node.exe" ( SET "_prog=%dp0%\node.exe" )
///   "%_prog%"  "%dp0%\node_modules\...\script.js" %*
///
/// 解析策略：
/// 1. 从 IF EXIST 或 SET 行提取 node.exe 的相对路径
/// 2. 从包含 %* 的执行行提取 .js/.mjs 脚本路径
#[cfg(target_os = "windows")]
fn parse_npm_cmd_file(cmd_path: &Path) -> Option<(String, String)> {
    let content = std::fs::read_to_string(cmd_path).ok()?;
    let cmd_dir = cmd_path.parent()?;

    let mut node_path: Option<String> = None;
    let mut script_path: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();

        if line.contains("node.exe") && node_path.is_none() {
            for part in line.split('"') {
                if part.contains("node.exe") && !part.contains("node_modules") {
                    let resolved = part
                        .replace("%dp0%\\", "")
                        .replace("%~dp0\\", "")
                        .replace("%dp0%/", "")
                        .replace("%~dp0/", "");
                    let full = cmd_dir.join(&resolved);
                    if full.exists() {
                        node_path = Some(full.to_string_lossy().to_string());
                        break;
                    }
                }
            }
        }

        if line.contains("%*") && (line.contains(".js") || line.contains(".mjs")) && script_path.is_none() {
            for part in line.split('"') {
                if part.ends_with(".js") || part.ends_with(".mjs") {
                    let resolved = part
                        .replace("%dp0%\\", "")
                        .replace("%~dp0\\", "")
                        .replace("%dp0%/", "")
                        .replace("%~dp0/", "");
                    let full = cmd_dir.join(&resolved);
                    if full.exists() {
                        script_path = Some(full.to_string_lossy().to_string());
                        break;
                    }
                }
            }
        }

        if node_path.is_some() && script_path.is_some() {
            break;
        }
    }

    match (node_path, script_path) {
        (Some(node), Some(script)) => Some((node, script)),
        _ => None,
    }
}

/// 创建一个能正确找到 CLI 可执行文件的 Command。
/// 在 Windows 上通过注册表 PATH 搜索可执行文件：
/// - 找到 .exe 文件时直接使用完整路径
/// - 找到 .cmd (npm包装脚本) 时，解析出底层 node.exe + JS 脚本，
///   直接通过 node.exe 调用，绕过 cmd.exe 的参数限制
pub fn create_command(program: &str) -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let path = Path::new(program);
        if !path.is_absolute() && !program.contains('\\') && !program.contains('/') {
            match resolve_in_path(program) {
                ResolvedProgram::Exe(exe_path) => {
                    let mut cmd = std::process::Command::new(&exe_path);
                    let merged = get_merged_path();
                    if !merged.is_empty() {
                        cmd.env("PATH", merged);
                    }
                    return cmd;
                }
                ResolvedProgram::CmdScript { node_exe, script } => {
                    let mut cmd = std::process::Command::new(&node_exe);
                    cmd.arg(&script);
                    let merged = get_merged_path();
                    if !merged.is_empty() {
                        cmd.env("PATH", merged);
                    }
                    return cmd;
                }
                ResolvedProgram::NotFound => {}
            }
        }

        let mut cmd = std::process::Command::new(program);
        let merged = get_merged_path();
        if !merged.is_empty() {
            cmd.env("PATH", merged);
        }
        cmd
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new(program)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum AgentEvent {
    Log { content: String, level: String },
    Progress { message: String },
    TokenUsage { input_tokens: u64, output_tokens: u64 },
    Completed { exit_code: i32, result: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOutput {
    pub text_response: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub model_used: Option<String>,
    pub session_id: Option<String>,
    pub exit_code: i32,
    pub duration_ms: u64,
}

pub struct ExecutionHandle {
    cancel_sender: tokio::sync::oneshot::Sender<()>,
    #[allow(dead_code)]
    child_pid: u32,
}

impl ExecutionHandle {
    pub fn cancel(self) {
        let _ = self.cancel_sender.send(());
    }
}

#[async_trait::async_trait]
pub trait AgentRunner: Send + Sync {
    fn build_command(
        &self,
        cli_path: &str,
        prompt: &str,
        working_dir: &Path,
        model: Option<&str>,
        allowed_tools: &[String],
    ) -> std::process::Command;

    fn parse_event_line(&self, line: &str) -> Option<AgentEvent>;

    fn extract_output(
        &self,
        events: &[AgentEvent],
        exit_code: i32,
        duration_ms: u64,
    ) -> AgentOutput;

    async fn get_version(&self, cli_path: &str) -> Result<String, String>;

    fn agent_type(&self) -> &str;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionState {
    pub task_id: String,
    pub status: String,
    pub logs: Vec<CachedLog>,
    pub result: Option<AgentOutput>,
    pub error: Option<String>,
    pub start_time_ms: u64,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedLog {
    pub content: String,
    pub level: String,
    pub timestamp_ms: u64,
}

pub struct AgentManager {
    runners: HashMap<String, Box<dyn AgentRunner>>,
    active_executions: Arc<Mutex<HashMap<String, ExecutionHandle>>>,
    execution_states: Arc<Mutex<HashMap<String, ExecutionState>>>,
}

impl AgentManager {
    pub fn new() -> Self {
        let mut runners: HashMap<String, Box<dyn AgentRunner>> = HashMap::new();
        runners.insert(
            "claude_code".to_string(),
            Box::new(ClaudeCodeRunner::new()),
        );
        runners.insert("codex".to_string(), Box::new(CodexRunner::new()));
        Self {
            runners,
            active_executions: Arc::new(Mutex::new(HashMap::new())),
            execution_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start_background_execution(
        &self,
        config: AgentConfig,
        prompt: String,
        project_path: String,
        task_id: String,
        app: tauri::AppHandle,
    ) -> Result<(), String> {
        let runner = self
            .runners
            .get(&config.agent_type)
            .ok_or_else(|| format!("不支持的 Agent 类型: {}", config.agent_type))?;

        let api_key = crypto::decrypt_api_key(&config.api_key_encrypted)?;
        let sandbox: SandboxConfig =
            serde_json::from_str(&config.sandbox_config).unwrap_or_default();

        let work_dir = if sandbox.enable_worktree_isolation {
            match WorktreeManager::create(&project_path, &task_id) {
                Ok(wt) => wt.path().to_string_lossy().to_string(),
                Err(_) => project_path.clone(),
            }
        } else {
            project_path.clone()
        };

        let mut cmd = runner.build_command(
            &config.cli_path,
            &prompt,
            Path::new(&work_dir),
            Some(&config.default_model),
            &sandbox.allowed_tools,
        );

        cmd.env(self.api_key_env_var(&config.agent_type), &api_key);

        if let Ok(env_map) = serde_json::from_str::<HashMap<String, String>>(&config.env_vars) {
            for (key, value) in &env_map {
                cmd.env(key, value);
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let state = ExecutionState {
            task_id: task_id.clone(),
            status: "running".to_string(),
            logs: Vec::new(),
            result: None,
            error: None,
            start_time_ms: now_ms,
            duration_ms: None,
        };
        self.execution_states
            .lock()
            .await
            .insert(task_id.clone(), state);

        let states = self.execution_states.clone();
        let timeout_secs = config.timeout_seconds as u64;
        let task_id_clone = task_id.clone();
        let event_name = format!("agent:log:{}", task_id);

        tokio::spawn(async move {
            let start = Instant::now();
            let run_result = Self::run_process(
                cmd,
                timeout_secs,
                states.clone(),
                &task_id_clone,
                app.clone(),
                &event_name,
            )
            .await;

            let duration_ms = start.elapsed().as_millis() as u64;
            let mut states_lock = states.lock().await;
            if let Some(state) = states_lock.get_mut(&task_id_clone) {
                state.duration_ms = Some(duration_ms);
                match run_result {
                    Ok(output) => {
                        state.status = "completed".to_string();
                        state.result = Some(output.clone());
                        let _ = app.emit(
                            &event_name,
                            AgentEvent::Completed {
                                exit_code: output.exit_code,
                                result: output.text_response.clone(),
                            },
                        );
                    }
                    Err(err) => {
                        state.status = "failed".to_string();
                        state.error = Some(err.clone());
                        let _ = app.emit(
                            &event_name,
                            AgentEvent::Failed { error: err },
                        );
                    }
                }
            }
        });

        Ok(())
    }

    async fn run_process(
        cmd: std::process::Command,
        timeout_secs: u64,
        states: Arc<Mutex<HashMap<String, ExecutionState>>>,
        task_id: &str,
        app: tauri::AppHandle,
        event_name: &str,
    ) -> Result<AgentOutput, String> {
        use tauri::Emitter;

        let mut tokio_cmd: TokioCommand = cmd.into();
        tokio_cmd.stdout(std::process::Stdio::piped());
        tokio_cmd.stderr(std::process::Stdio::piped());

        let mut child = tokio_cmd
            .spawn()
            .map_err(|e| format!("启动 Agent 进程失败: {}", e))?;

        let stdout = child.stdout.take().ok_or("无法获取 stdout")?;
        let stderr = child.stderr.take().ok_or("无法获取 stderr")?;
        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let states_stderr = states.clone();
        let task_id_stderr = task_id.to_string();
        let app_stderr = app.clone();
        let event_name_stderr = event_name.to_string();
        let stderr_handle = tokio::spawn(async move {
            let mut stderr_lines = Vec::new();
            while let Ok(Some(line)) = stderr_reader.next_line().await {
                stderr_lines.push(line.clone());
                let event = AgentEvent::Log {
                    content: line.clone(),
                    level: "stderr".to_string(),
                };
                let _ = app_stderr.emit(&event_name_stderr, &event);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut lock = states_stderr.lock().await;
                if let Some(s) = lock.get_mut(&task_id_stderr) {
                    s.logs.push(CachedLog {
                        content: line,
                        level: "stderr".to_string(),
                        timestamp_ms: now,
                    });
                }
            }
            stderr_lines
        });

        let states_stdout = states.clone();
        let task_id_stdout = task_id.to_string();
        let app_stdout = app.clone();
        let event_name_stdout = event_name.to_string();

        let timeout = tokio::time::Duration::from_secs(timeout_secs);
        let mut collected_events: Vec<AgentEvent> = Vec::new();

        let result = tokio::time::timeout(timeout, async {
            while let Ok(Some(line)) = stdout_reader.next_line().await {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                    let event_type = json["type"].as_str().unwrap_or("");
                    let log_event = AgentEvent::Log {
                        content: line.clone(),
                        level: "stdout".to_string(),
                    };
                    let _ = app_stdout.emit(&event_name_stdout, &log_event);

                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let mut lock = states_stdout.lock().await;
                    if let Some(s) = lock.get_mut(&task_id_stdout) {
                        s.logs.push(CachedLog {
                            content: line.clone(),
                            level: "stdout".to_string(),
                            timestamp_ms: now,
                        });
                    }
                    drop(lock);

                    if event_type == "turn.completed" || event_type == "result" {
                        let usage_event = if event_type == "turn.completed" {
                            let usage = &json["usage"];
                            Some(AgentEvent::TokenUsage {
                                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                            })
                        } else {
                            None
                        };
                        if let Some(evt) = usage_event {
                            let _ = app_stdout.emit(&event_name_stdout, &evt);
                            collected_events.push(evt);
                        }
                    }
                    collected_events.push(log_event);
                }
            }
            child.wait().await
        })
        .await;

        let stderr_lines = stderr_handle.await.unwrap_or_default();

        match result {
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);
                let duration_ms = 0u64;
                if exit_code != 0 && collected_events.is_empty() && !stderr_lines.is_empty() {
                    return Err(format!(
                        "Agent 进程退出码 {}:\n{}",
                        exit_code,
                        stderr_lines.join("\n")
                    ));
                }
                let mut text_response = String::new();
                let mut total_input = 0u64;
                let mut total_output = 0u64;
                for evt in &collected_events {
                    match evt {
                        AgentEvent::Log { content, .. } => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
                                let t = json["type"].as_str().unwrap_or("");
                                match t {
                                    "item.started" | "item.completed" => {
                                        let item = &json["item"];
                                        let item_type = item["type"]
                                            .as_str()
                                            .or_else(|| item["item_type"].as_str())
                                            .unwrap_or("");
                                        match item_type {
                                            "agent_message" | "assistant_message" => {
                                                if let Some(text) = item["text"].as_str() {
                                                    if !text_response.is_empty() {
                                                        text_response.push('\n');
                                                    }
                                                    text_response.push_str(text);
                                                }
                                            }
                                            "command_execution" => {
                                                if let Some(cmd_str) = item["command"].as_str() {
                                                    if !text_response.is_empty() {
                                                        text_response.push('\n');
                                                    }
                                                    text_response.push_str(&format!("$ {}", cmd_str));
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    "result" => {
                                        if let Some(text) = json["result"].as_str() {
                                            text_response = text.to_string();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        AgentEvent::TokenUsage { input_tokens, output_tokens } => {
                            total_input += input_tokens;
                            total_output += output_tokens;
                        }
                        _ => {}
                    }
                }
                Ok(AgentOutput {
                    text_response,
                    input_tokens: Some(total_input),
                    output_tokens: Some(total_output),
                    estimated_cost_usd: None,
                    model_used: None,
                    session_id: None,
                    exit_code,
                    duration_ms,
                })
            }
            Ok(Err(e)) => Err(format!("Agent 进程异常: {}", e)),
            Err(_) => {
                let _ = child.kill().await;
                Err(format!("Agent 执行超时（{}秒）", timeout_secs))
            }
        }
    }

    pub async fn get_execution_state(&self, task_id: &str) -> Option<ExecutionState> {
        self.execution_states.lock().await.get(task_id).cloned()
    }

    fn api_key_env_var(&self, agent_type: &str) -> &str {
        match agent_type {
            "claude_code" => "ANTHROPIC_API_KEY",
            "codex" => "OPENAI_API_KEY",
            _ => "API_KEY",
        }
    }

    pub async fn check_health(&self, config: &AgentConfig) -> AgentHealthStatus {
        let runner = match self.runners.get(&config.agent_type) {
            Some(r) => r,
            None => {
                return AgentHealthStatus {
                    agent_id: config.id,
                    cli_found: false,
                    detected_version: None,
                    version_compatible: false,
                    status: "error".to_string(),
                    message: Some(format!("不支持的 Agent 类型: {}", config.agent_type)),
                }
            }
        };

        let version_result = runner.get_version(&config.cli_path).await;
        let (cli_found, detected_version) = match version_result {
            Ok(v) => (true, Some(v)),
            Err(_) => (false, None),
        };

        let version_compatible = if let Some(ref ver) = detected_version {
            if config.min_cli_version.is_empty() {
                true
            } else {
                ver >= &config.min_cli_version
            }
        } else {
            false
        };

        let api_key_configured = !config.api_key_encrypted.is_empty();

        let status = if !cli_found {
            "unavailable"
        } else if !version_compatible {
            "outdated"
        } else if !api_key_configured {
            "no_key"
        } else {
            "healthy"
        };

        let message = match status {
            "unavailable" => format!("CLI 未安装或不在 PATH 中: {}", config.cli_path),
            "outdated" => format!(
                "版本过低，当前 {}，要求 >= {}",
                detected_version.as_deref().unwrap_or("?"),
                config.min_cli_version
            ),
            "no_key" => "API Key 未配置".to_string(),
            "healthy" => format!(
                "就绪 (v{})",
                detected_version.as_deref().unwrap_or("?")
            ),
            _ => String::new(),
        };

        AgentHealthStatus {
            agent_id: config.id,
            cli_found,
            detected_version,
            version_compatible,
            status: status.to_string(),
            message: Some(message),
        }
    }

    pub async fn cancel_execution(&self, execution_id: &str) -> Result<(), String> {
        let mut executions = self.active_executions.lock().await;
        if let Some(handle) = executions.remove(execution_id) {
            handle.cancel();
            let mut states = self.execution_states.lock().await;
            if let Some(state) = states.get_mut(execution_id) {
                state.status = "cancelled".to_string();
            }
            Ok(())
        } else {
            Err("未找到执行中的任务".to_string())
        }
    }
}
