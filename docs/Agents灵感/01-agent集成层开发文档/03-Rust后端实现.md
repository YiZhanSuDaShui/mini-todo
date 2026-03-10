# Agent 集成层 - Rust 后端实现

## 1. 模块结构

```
src-tauri/src/
├── services/
│   ├── mod.rs              // 追加 pub mod agent;
│   ├── agent/
│   │   ├── mod.rs          // 模块入口，导出公共接口
│   │   ├── runner.rs       // AgentRunner Trait + AgentManager
│   │   ├── claude_code.rs  // Claude Code CLI 适配器
│   │   ├── codex.rs        // Codex CLI 适配器
│   │   ├── crypto.rs       // API Key 加密/解密
│   │   └── worktree.rs     // Git Worktree 管理
│   └── ...
├── commands/
│   ├── mod.rs              // 追加 pub mod agent_cmd;
│   ├── agent_cmd.rs        // Agent Tauri 命令
│   └── ...
└── db/
    ├── mod.rs              // 追加 pub mod agent_db;
    ├── agent_db.rs         // Agent 数据库操作
    └── ...
```

## 2. services/agent/mod.rs

```rust
pub mod runner;
pub mod claude_code;
pub mod codex;
pub mod crypto;
pub mod worktree;

pub use runner::{AgentRunner, AgentManager, AgentOutput, ExecutionHandle};
pub use crypto::{encrypt_api_key, decrypt_api_key};
pub use worktree::WorktreeManager;
```

## 3. AgentRunner Trait (runner.rs)

### 3.1 核心 Trait

```rust
use std::path::Path;
use tokio::sync::mpsc;

/// Agent 执行过程中的事件
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum AgentEvent {
    /// Agent 输出的文本日志
    Log { content: String, level: String },
    /// Agent 执行进度更新
    Progress { message: String },
    /// Token 使用统计（执行过程中或完成时）
    TokenUsage { input_tokens: u64, output_tokens: u64 },
    /// 执行完成
    Completed { exit_code: i32, result: String },
    /// 执行失败
    Failed { error: String },
}

/// Agent 的标准化输出
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// 执行句柄，用于取消执行
pub struct ExecutionHandle {
    cancel_sender: tokio::sync::oneshot::Sender<()>,
    child_pid: u32,
}

impl ExecutionHandle {
    pub fn cancel(self) {
        let _ = self.cancel_sender.send(());
    }
}

/// CLI 适配器 Trait
#[async_trait::async_trait]
pub trait AgentRunner: Send + Sync {
    /// 构建 CLI 命令参数（cli_path 来自 AgentConfig.cli_path）
    fn build_command(
        &self,
        cli_path: &str,
        prompt: &str,
        working_dir: &Path,
        model: Option<&str>,
        allowed_tools: &[String],
    ) -> std::process::Command;

    /// 解析 JSONL 事件行，返回标准化事件
    fn parse_event_line(&self, line: &str) -> Option<AgentEvent>;

    /// 从最终输出中提取标准化结果
    fn extract_output(&self, events: &[AgentEvent], exit_code: i32, duration_ms: u64) -> AgentOutput;

    /// 获取 CLI 版本号
    async fn get_version(&self, cli_path: &str) -> Result<String, String>;

    /// 获取 Agent 类型标识
    fn agent_type(&self) -> &str;
}
```

### 3.2 AgentManager（统一管理器）

```rust
use crate::db::models::AgentConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AgentManager {
    runners: HashMap<String, Box<dyn AgentRunner>>,
    active_executions: Arc<Mutex<HashMap<String, ExecutionHandle>>>,
}

impl AgentManager {
    pub fn new() -> Self {
        let mut runners: HashMap<String, Box<dyn AgentRunner>> = HashMap::new();
        runners.insert("claude_code".to_string(), Box::new(claude_code::ClaudeCodeRunner::new()));
        runners.insert("codex".to_string(), Box::new(codex::CodexRunner::new()));
        Self {
            runners,
            active_executions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 执行 Agent 任务
    pub async fn execute(
        &self,
        config: &AgentConfig,
        prompt: &str,
        project_path: &str,
        task_id: &str,
        event_sender: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentOutput, String> {
        let runner = self.runners.get(&config.agent_type)
            .ok_or_else(|| format!("不支持的 Agent 类型: {}", config.agent_type))?;

        let api_key = crypto::decrypt_api_key(&config.api_key_encrypted)?;
        let sandbox: SandboxConfig = serde_json::from_str(&config.sandbox_config)
            .unwrap_or_default();
        
        // 创建 Worktree（如果启用）
        let work_dir = if sandbox.enable_worktree_isolation {
            let wt = WorktreeManager::create(project_path, task_id)?;
            wt.path().to_string_lossy().to_string()
        } else {
            project_path.to_string()
        };

        // 构建并执行命令（cli_path 来自配置）
        let mut cmd = runner.build_command(
            &config.cli_path,
            prompt,
            Path::new(&work_dir),
            Some(&config.default_model),
            &sandbox.allowed_tools,
        );
        
        // 注入 API Key
        cmd.env(self.api_key_env_var(&config.agent_type), &api_key);

        // 执行并流式处理输出
        let output = self.run_with_streaming(
            cmd,
            runner.as_ref(),
            config.timeout_seconds as u64,
            event_sender,
        ).await?;

        Ok(output)
    }

    /// 运行命令并流式处理 JSONL 输出
    async fn run_with_streaming(
        &self,
        mut cmd: std::process::Command,
        runner: &dyn AgentRunner,
        timeout_secs: u64,
        event_sender: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<AgentOutput, String> {
        // 实现细节见下方 3.3
        todo!()
    }

    fn api_key_env_var(&self, agent_type: &str) -> &str {
        match agent_type {
            "claude_code" => "ANTHROPIC_API_KEY",
            "codex" => "CODEX_API_KEY",
            _ => "API_KEY",
        }
    }

    /// 健康检查
    pub async fn check_health(&self, config: &AgentConfig) -> AgentHealthStatus {
        // 1. 检查 CLI 是否存在
        // 2. 获取版本号
        // 3. 比对最低版本要求
        // 4. 返回状态
        todo!()
    }

    /// 取消执行
    pub async fn cancel_execution(&self, execution_id: &str) -> Result<(), String> {
        let mut executions = self.active_executions.lock().await;
        if let Some(handle) = executions.remove(execution_id) {
            handle.cancel();
            Ok(())
        } else {
            Err("未找到执行中的任务".to_string())
        }
    }
}
```

### 3.3 流式执行核心逻辑

```rust
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use std::time::Instant;

async fn run_with_streaming(
    &self,
    cmd: std::process::Command,
    runner: &dyn AgentRunner,
    timeout_secs: u64,
    event_sender: mpsc::UnboundedSender<AgentEvent>,
) -> Result<AgentOutput, String> {
    let start = Instant::now();
    let mut collected_events: Vec<AgentEvent> = Vec::new();

    // 将 std::process::Command 转换为 tokio::process::Command
    let mut tokio_cmd = TokioCommand::from(cmd);
    tokio_cmd.stdout(std::process::Stdio::piped());
    tokio_cmd.stderr(std::process::Stdio::piped());

    let mut child = tokio_cmd.spawn()
        .map_err(|e| format!("启动 Agent 进程失败: {}", e))?;

    let stdout = child.stdout.take()
        .ok_or("无法获取 stdout")?;

    let mut reader = BufReader::new(stdout).lines();

    // 设置超时
    let timeout = tokio::time::Duration::from_secs(timeout_secs);

    let result = tokio::time::timeout(timeout, async {
        while let Ok(Some(line)) = reader.next_line().await {
            // 解析 JSONL 事件
            if let Some(event) = runner.parse_event_line(&line) {
                // 推送到前端
                let _ = event_sender.send(event.clone());
                collected_events.push(event);
            }
        }
        child.wait().await
    }).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(status)) => {
            let exit_code = status.code().unwrap_or(-1);
            let output = runner.extract_output(&collected_events, exit_code, duration_ms);
            Ok(output)
        }
        Ok(Err(e)) => Err(format!("Agent 进程异常: {}", e)),
        Err(_) => {
            // 超时，杀死进程
            let _ = child.kill().await;
            Err(format!("Agent 执行超时（{}秒）", timeout_secs))
        }
    }
}
```

## 4. Claude Code 适配器 (claude_code.rs)

```rust
use super::runner::{AgentRunner, AgentEvent, AgentOutput};
use std::path::Path;
use std::process::Command;

pub struct ClaudeCodeRunner;

impl ClaudeCodeRunner {
    pub fn new() -> Self { Self }
}

#[async_trait::async_trait]
impl AgentRunner for ClaudeCodeRunner {
    fn build_command(
        &self,
        cli_path: &str,
        prompt: &str,
        working_dir: &Path,
        model: Option<&str>,
        allowed_tools: &[String],
    ) -> Command {
        let mut cmd = Command::new(cli_path);
        cmd.current_dir(working_dir);
        cmd.args(["-p", prompt]);
        cmd.args(["--output-format", "stream-json"]);
        cmd.arg("--verbose");
        cmd.arg("--dangerously-skip-permissions");

        if let Some(m) = model {
            cmd.args(["--model", m]);
        }
        if !allowed_tools.is_empty() {
            cmd.args(["--allowedTools", &allowed_tools.join(",")]);
        }
        cmd
    }

    fn parse_event_line(&self, line: &str) -> Option<AgentEvent> {
        let json: serde_json::Value = serde_json::from_str(line).ok()?;
        
        match json["type"].as_str()? {
            "stream_event" => {
                if let Some(text) = json["event"]["delta"]["text"].as_str() {
                    Some(AgentEvent::Log {
                        content: text.to_string(),
                        level: "stdout".to_string(),
                    })
                } else {
                    None
                }
            }
            "result" => {
                let result_text = json["result"].as_str().unwrap_or("").to_string();
                let cost = json["cost"]["total_cost_usd"].as_f64();
                let session_id = json["session_id"].as_str().map(String::from);
                let model_used = json["model"].as_str().map(String::from);
                Some(AgentEvent::Completed {
                    exit_code: 0,
                    result: serde_json::json!({
                        "text": result_text,
                        "cost_usd": cost,
                        "session_id": session_id,
                        "model": model_used,
                    }).to_string(),
                })
            }
            _ => None,
        }
    }

    fn extract_output(&self, events: &[AgentEvent], exit_code: i32, duration_ms: u64) -> AgentOutput {
        let mut text_response = String::new();
        let mut cost: Option<f64> = None;
        let mut session_id: Option<String> = None;
        let mut model_used: Option<String> = None;

        for event in events {
            match event {
                AgentEvent::Log { content, .. } => text_response.push_str(content),
                AgentEvent::Completed { result, .. } => {
                    // result 中包含 JSON 编码的完整数据
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(result) {
                        if let Some(text) = data["text"].as_str() {
                            if !text.is_empty() {
                                text_response = text.to_string();
                            }
                        }
                        cost = data["cost_usd"].as_f64();
                        session_id = data["session_id"].as_str().map(String::from);
                        model_used = data["model"].as_str().map(String::from);
                    }
                }
                _ => {}
            }
        }

        AgentOutput {
            text_response,
            input_tokens: None,
            output_tokens: None,
            estimated_cost_usd: cost,
            model_used,
            session_id,
            exit_code,
            duration_ms,
        }
    }

    async fn get_version(&self, cli_path: &str) -> Result<String, String> {
        let output = Command::new(cli_path)
            .arg("--version")
            .output()
            .map_err(|e| format!("无法执行 {}: {}", cli_path, e))?;
        
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    }

    fn agent_type(&self) -> &str { "claude_code" }
}
```

## 5. Codex 适配器 (codex.rs)

```rust
pub struct CodexRunner;

impl CodexRunner {
    pub fn new() -> Self { Self }
}

#[async_trait::async_trait]
impl AgentRunner for CodexRunner {
    fn build_command(
        &self,
        cli_path: &str,
        prompt: &str,
        working_dir: &Path,
        model: Option<&str>,
        _allowed_tools: &[String],
    ) -> Command {
        let mut cmd = Command::new(cli_path);
        cmd.current_dir(working_dir);
        cmd.args(["exec", "--json", "--full-auto"]);
        cmd.args(["--sandbox", "workspace-write"]);
        cmd.arg(prompt);
        cmd
    }

    fn parse_event_line(&self, line: &str) -> Option<AgentEvent> {
        let json: serde_json::Value = serde_json::from_str(line).ok()?;
        
        match json["type"].as_str()? {
            "item.started" | "item.completed" => {
                let item = &json["item"];
                // 兼容 "type" 和 "item_type" 两种格式
                let item_type = item["type"].as_str()
                    .or_else(|| item["item_type"].as_str())
                    .unwrap_or("unknown");
                
                let content = match item_type {
                    "agent_message" | "assistant_message" => {
                        item["text"].as_str().unwrap_or("").to_string()
                    }
                    "command_execution" => {
                        format!("$ {}", item["command"].as_str().unwrap_or(""))
                    }
                    _ => return None,
                };
                
                Some(AgentEvent::Log {
                    content,
                    level: "stdout".to_string(),
                })
            }
            "turn.completed" => {
                let usage = &json["usage"];
                let input = usage["input_tokens"].as_u64().unwrap_or(0);
                let output = usage["output_tokens"].as_u64().unwrap_or(0);
                Some(AgentEvent::TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                })
            }
            "turn.failed" => {
                Some(AgentEvent::Failed {
                    error: json.to_string(),
                })
            }
            _ => None,
        }
    }

    fn extract_output(&self, events: &[AgentEvent], exit_code: i32, duration_ms: u64) -> AgentOutput {
        let mut text_response = String::new();
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;

        for event in events {
            match event {
                AgentEvent::Log { content, .. } => {
                    if !text_response.is_empty() {
                        text_response.push('\n');
                    }
                    text_response.push_str(content);
                }
                AgentEvent::TokenUsage { input_tokens, output_tokens } => {
                    total_input += input_tokens;
                    total_output += output_tokens;
                }
                _ => {}
            }
        }

        AgentOutput {
            text_response,
            input_tokens: Some(total_input),
            output_tokens: Some(total_output),
            estimated_cost_usd: None,
            model_used: None,
            session_id: None,
            exit_code,
            duration_ms,
        }
    }

    async fn get_version(&self, cli_path: &str) -> Result<String, String> {
        let output = Command::new(cli_path)
            .arg("--version")
            .output()
            .map_err(|e| format!("无法执行 {}: {}", cli_path, e))?;
        
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    }

    fn agent_type(&self) -> &str { "codex" }
}
```

## 6. API Key 加密 (crypto.rs)

```rust
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit};
use rand::Rng;

/// 从机器唯一标识派生 AES-256 密钥
fn derive_key() -> Result<Key<Aes256Gcm>, String> {
    // 获取机器唯一标识
    let machine_id = machine_uid::get()
        .map_err(|e| format!("无法获取机器标识: {}", e))?;
    
    // SHA-256 哈希得到 32 字节密钥
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(machine_id.as_bytes());
    hasher.update(b"mini-todo-agent-key-salt");
    let result = hasher.finalize();
    
    Ok(*Key::<Aes256Gcm>::from_slice(&result))
}

/// 加密 API Key
/// 返回格式：base64(nonce + ciphertext)
pub fn encrypt_api_key(plain_key: &str) -> Result<String, String> {
    if plain_key.is_empty() {
        return Ok(String::new());
    }
    
    let key = derive_key()?;
    let cipher = Aes256Gcm::new(&key);
    
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    
    let ciphertext = cipher.encrypt(nonce, plain_key.as_bytes())
        .map_err(|e| format!("加密失败: {}", e))?;
    
    // nonce (12 bytes) + ciphertext
    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);
    
    Ok(base64::engine::general_purpose::STANDARD.encode(&combined))
}

/// 解密 API Key
pub fn decrypt_api_key(encrypted: &str) -> Result<String, String> {
    if encrypted.is_empty() {
        return Ok(String::new());
    }
    
    let combined = base64::engine::general_purpose::STANDARD.decode(encrypted)
        .map_err(|e| format!("Base64 解码失败: {}", e))?;
    
    if combined.len() < 13 {
        return Err("加密数据格式无效".to_string());
    }
    
    let key = derive_key()?;
    let cipher = Aes256Gcm::new(&key);
    let nonce = Nonce::from_slice(&combined[..12]);
    
    let plaintext = cipher.decrypt(nonce, &combined[12..])
        .map_err(|e| format!("解密失败（可能是不同机器的数据）: {}", e))?;
    
    String::from_utf8(plaintext)
        .map_err(|e| format!("UTF-8 解码失败: {}", e))
}
```

> 注意：`sha2` crate 需要额外添加到 Cargo.toml 依赖中。

## 7. Git Worktree 管理 (worktree.rs)

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WorktreeManager;

pub struct Worktree {
    path: PathBuf,
    branch_name: String,
    project_path: PathBuf,
}

impl WorktreeManager {
    /// 创建新的 Worktree
    pub fn create(project_path: &str, task_id: &str) -> Result<Worktree, String> {
        let project = Path::new(project_path);
        let branch_name = format!("agent/{}", task_id);
        let worktree_dir = project.join(".agent-worktrees").join(task_id);

        // 确保 .agent-worktrees 目录存在
        std::fs::create_dir_all(worktree_dir.parent().unwrap())
            .map_err(|e| format!("创建 worktree 目录失败: {}", e))?;

        // git worktree add {path} -b {branch} HEAD
        let output = Command::new("git")
            .current_dir(project)
            .args(["worktree", "add"])
            .arg(&worktree_dir)
            .args(["-b", &branch_name, "HEAD"])
            .output()
            .map_err(|e| format!("创建 worktree 失败: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git worktree add 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(Worktree {
            path: worktree_dir,
            branch_name,
            project_path: project.to_path_buf(),
        })
    }

    /// 收集 Worktree 中的变更
    pub fn collect_diff(worktree: &Worktree) -> Result<String, String> {
        // 先 add 所有变更
        let _ = Command::new("git")
            .current_dir(&worktree.path)
            .args(["add", "-A"])
            .output();

        let output = Command::new("git")
            .current_dir(&worktree.path)
            .args(["diff", "--cached"])
            .output()
            .map_err(|e| format!("获取 diff 失败: {}", e))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// 获取变更文件列表
    pub fn changed_files(worktree: &Worktree) -> Result<Vec<String>, String> {
        let output = Command::new("git")
            .current_dir(&worktree.path)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .map_err(|e| format!("获取变更文件列表失败: {}", e))?;

        let files = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();

        Ok(files)
    }

    /// 清理 Worktree 和分支
    pub fn cleanup(worktree: &Worktree) -> Result<(), String> {
        // 删除 worktree
        let _ = Command::new("git")
            .current_dir(&worktree.project_path)
            .args(["worktree", "remove", "--force"])
            .arg(&worktree.path)
            .output();

        // 删除分支
        let _ = Command::new("git")
            .current_dir(&worktree.project_path)
            .args(["branch", "-D", &worktree.branch_name])
            .output();

        // 清理目录（如果 worktree remove 失败）
        if worktree.path.exists() {
            let _ = std::fs::remove_dir_all(&worktree.path);
        }

        Ok(())
    }

    /// 将 Agent 分支合并回主分支
    pub fn merge_to_current(worktree: &Worktree) -> Result<String, String> {
        // 先在 worktree 中提交
        let _ = Command::new("git")
            .current_dir(&worktree.path)
            .args(["add", "-A"])
            .output();

        let commit_output = Command::new("git")
            .current_dir(&worktree.path)
            .args(["commit", "-m", &format!("[agent] task completed")])
            .output()
            .map_err(|e| format!("提交失败: {}", e))?;

        // 在主仓库中合并
        let merge_output = Command::new("git")
            .current_dir(&worktree.project_path)
            .args(["merge", &worktree.branch_name, "--no-ff", "-m",
                   &format!("[agent] merge {}", worktree.branch_name)])
            .output()
            .map_err(|e| format!("合并失败: {}", e))?;

        if !merge_output.status.success() {
            return Err(format!(
                "合并冲突: {}",
                String::from_utf8_lossy(&merge_output.stderr)
            ));
        }

        // 获取合并后的 commit hash
        let hash = Command::new("git")
            .current_dir(&worktree.project_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        Ok(hash)
    }
}

impl Worktree {
    pub fn path(&self) -> &Path { &self.path }
    pub fn branch_name(&self) -> &str { &self.branch_name }
}
```

## 8. Tauri 命令 (commands/agent_cmd.rs)

```rust
use tauri::State;
use crate::db::Database;
use crate::db::agent_db;
use crate::db::models::{AgentConfig, CreateAgentRequest, UpdateAgentRequest, AgentHealthStatus};
use crate::services::agent::{encrypt_api_key, AgentManager};

/// 获取所有 Agent 配置
#[tauri::command]
pub fn get_agents(db: State<'_, Database>) -> Result<Vec<AgentConfig>, String> {
    db.with_connection(|conn| agent_db::get_all_agents(conn))
        .map_err(|e| e.to_string())
}

/// 创建 Agent 配置
#[tauri::command]
pub fn create_agent(
    db: State<'_, Database>,
    request: CreateAgentRequest,
) -> Result<i64, String> {
    let encrypted_key = if let Some(ref key) = request.api_key {
        encrypt_api_key(key)?
    } else {
        String::new()
    };
    
    db.with_connection(|conn| agent_db::create_agent(conn, &request, &encrypted_key))
        .map_err(|e| e.to_string())
}

/// 更新 Agent 配置
#[tauri::command]
pub fn update_agent(
    db: State<'_, Database>,
    id: i64,
    request: UpdateAgentRequest,
) -> Result<(), String> {
    let encrypted_key = match &request.api_key {
        Some(key) if !key.is_empty() => Some(encrypt_api_key(key)?),
        Some(_) => Some(String::new()),  // 空字符串 = 清除 Key
        None => None,                     // None = 不修改
    };
    
    db.with_connection(|conn| {
        agent_db::update_agent(conn, id, &request, encrypted_key.as_deref())
    }).map_err(|e| e.to_string())
}

/// 删除 Agent 配置
#[tauri::command]
pub fn delete_agent(db: State<'_, Database>, id: i64) -> Result<(), String> {
    db.with_connection(|conn| agent_db::delete_agent(conn, id))
        .map_err(|e| e.to_string())
}

/// 检查 Agent 健康状态
#[tauri::command]
pub async fn check_agent_health(
    db: State<'_, Database>,
    agent_manager: State<'_, AgentManager>,
    id: i64,
) -> Result<AgentHealthStatus, String> {
    let config = db.with_connection(|conn| agent_db::get_agent_by_id(conn, id))
        .map_err(|e| e.to_string())?;
    
    Ok(agent_manager.check_health(&config).await)
}

/// 检查所有 Agent 的健康状态
#[tauri::command]
pub async fn check_all_agents_health(
    db: State<'_, Database>,
    agent_manager: State<'_, AgentManager>,
) -> Result<Vec<AgentHealthStatus>, String> {
    let agents = db.with_connection(|conn| agent_db::get_all_agents(conn))
        .map_err(|e| e.to_string())?;
    
    let mut results = Vec::new();
    for agent in &agents {
        results.push(agent_manager.check_health(agent).await);
    }
    Ok(results)
}
```

## 9. lib.rs 变更

在 `lib.rs` 中新增：

```rust
// 1. 新增导入
use services::agent::AgentManager;
use commands::{
    // ... 现有命令 ...
    // 新增 Agent 命令
    get_agents,
    create_agent,
    update_agent,
    delete_agent,
    check_agent_health,
    check_all_agents_health,
};

// 2. 在 run() 函数中，创建 AgentManager 并注册
pub fn run() {
    let database = Database::new().expect("Failed to initialize database");
    let agent_manager = AgentManager::new();

    tauri::Builder::default()
        // ...
        .manage(database)
        .manage(agent_manager)  // 新增
        // ...
        .invoke_handler(tauri::generate_handler![
            // ... 现有命令 ...
            // Agent 命令（新增）
            get_agents,
            create_agent,
            update_agent,
            delete_agent,
            check_agent_health,
            check_all_agents_health,
        ])
        // ...
}
```

## 10. Cargo.toml 新增依赖

```toml
[dependencies]
# ... 现有依赖 ...
aes-gcm = "0.10"     # AES-256-GCM 加密（API Key 安全存储）
sha2 = "0.10"         # SHA-256 哈希（密钥派生）
rand = "0.8"          # 加密随机数（Nonce 生成）
machine-uid = "0.5"   # 获取机器唯一标识（密钥派生种子）
async-trait = "0.1"   # 异步 Trait 支持（AgentRunner Trait）
```
