# 01 - Agent 集成层

## 1. 概述

Agent 集成层是整个调度系统的基础设施，负责管理和连接各种 AI 代码编辑工具。核心目标是让系统能够灵活接入不同的 Agent，并统一它们的调用方式。

## 2. 支持的 Agent 类型

### 2.1 Claude Code

| 属性 | 说明 |
|------|------|
| 调用方式 | CLI (`claude` 命令) |
| 主要能力 | 复杂架构设计、多文件重构、深度代码理解 |
| 输出格式 | JSON 结构化输出（`--output-format json`） |
| 非交互模式 | `--print` 模式直接输出结果 |
| 模型选择 | 支持 `--model` 指定模型（sonnet / opus / haiku 等） |
| 权限控制 | `--allowedTools` 白名单、`--dangerously-skip-permissions` 跳过交互式审批 |
| 上下文支持 | 自动读取项目中的 CLAUDE.md 等配置文件 |

### 2.2 Codex (OpenAI)

| 属性 | 说明 |
|------|------|
| 调用方式 | CLI (`codex` 命令) |
| 主要能力 | 快速代码生成、小范围修改 |
| 输出格式 | 文本 + 代码块 |
| 非交互模式 | 支持非交互模式 |
| 特点 | 速度较快，适合简单明确的任务 |

### 2.3 未来可扩展（通过 CLI 适配器接入）

| Agent | CLI 命令 | 说明 |
|-------|---------|------|
| OpenCode | `opencode` | 开源 AI 编程助手 |
| Aider | `aider` | 基于 LLM 的终端编程助手 |
| 其他 | 自定义 | 任何提供 CLI 接口的 AI 编程工具 |

> 新 Agent 接入时需实现 CLI 适配器接口：命令行参数构建、输出解析、版本检测。

## 3. Agent 配置管理

### 3.1 配置模型

```
AgentConfig {
    id: number                          // 自增主键
    name: string                        // 显示名称
    agent_type: AgentType               // claude_code | codex | custom
    cli_path: string                    // CLI 可执行文件路径
    cli_version: string                 // CLI 版本号（健康检查时更新）
    api_key_encrypted: string           // API Key（AES 加密存储）
    default_model: string               // 默认使用的模型（如 claude-sonnet-4-20250514）
    max_concurrent: number              // 最大并发任务数
    timeout_seconds: number             // 默认超时时间
    env_vars: Record<string, string>    // 额外环境变量
    capabilities: string[]              // 能力标签（如 refactor, test, review）
    sandbox_config: SandboxConfig       // 沙盒隔离配置
    enabled: boolean                    // 是否启用
}
```

### 3.2 配置界面灵感

- 在设置页面新增 "Agent 管理" Tab
- 列表展示已配置的 Agent，卡片式布局
- 每张卡片显示：名称、类型图标、状态指示灯（在线/离线）、能力标签
- 支持添加、编辑、删除、启用/禁用操作
- 提供 "测试连接" 按钮，验证 CLI 可用性

### 3.3 API Key 安全存储

**方案：AES 加密存储在 SQLite 中**

- 使用 AES-256-GCM 加密 API Key 后存入 `api_key_encrypted` 字段
- 加密密钥派生自机器唯一标识（如 Machine GUID），确保跨机器不可解密
- 界面上以 `sk-****` 遮盖显示
- 相比 Windows Credential Manager，此方案更简单且随数据库导出时自动跳过（加密 Key 在其他机器上无法解密）

## 4. Agent 能力画像

### 4.1 能力维度（MVP 精简版）

MVP 阶段使用 5 个核心维度，避免过度设计：

| 维度 | 说明 | 量化范围 |
|------|------|---------|
| 代码生成 | 从零编写新功能 + 多文件操作 | 1~10 |
| 代码修复 | Bug 修复 + 代码重构 | 1~10 |
| 测试与审查 | 编写测试 + Code Review | 1~10 |
| 速度 | 执行速度 | 1~10 |
| 成本效率 | Token 消耗 / 单位产出 | 1~10 |

> 使用 1-10 的数值而非"高/中/低"，便于智能分配算法计算匹配分数。

### 4.2 默认能力预设

| Agent | 代码生成 | 代码修复 | 测试与审查 | 速度 | 成本效率 |
|-------|---------|---------|-----------|------|---------|
| Claude Code (Opus) | 9 | 9 | 8 | 4 | 3 |
| Claude Code (Sonnet) | 8 | 8 | 7 | 7 | 7 |
| Codex | 6 | 6 | 5 | 8 | 8 |

### 4.3 能力更新策略

- **初始值**：根据 Agent 类型和模型预设默认值（见上表）
- **动态更新**：根据历史执行结果的评分加权移动平均更新
- **用户覆盖**：用户可手动修改，手动值优先级最高

## 5. 通信协议

### 5.1 统一调用接口（CLI 适配器 Trait）

```rust
trait AgentRunner {
    /// 启动一个 Agent 任务，返回执行句柄
    async fn execute(&self, task: AgentTask) -> Result<AgentExecution>;
    
    /// 获取执行状态
    async fn status(&self, execution_id: &str) -> Result<ExecutionStatus>;
    
    /// 取消正在执行的任务
    async fn cancel(&self, execution_id: &str) -> Result<()>;
    
    /// 获取实时输出流
    fn output_stream(&self, execution_id: &str) -> impl Stream<Item = OutputLine>;
    
    /// 解析 Agent 输出，提取标准化结果
    fn parse_output(&self, raw_output: &str) -> Result<AgentOutput>;
    
    /// 获取 CLI 版本号
    async fn version(&self) -> Result<String>;
}
```

### 5.2 标准化输出结构

不同 Agent 的输出格式各异，适配器需将其解析为统一结构：

```rust
struct AgentOutput {
    text_response: String,         // Agent 的文本回复/摘要
    files_changed: Vec<FileChange>, // 变更的文件列表
    tokens_used: Option<TokenUsage>, // Token 使用统计（如果可获取）
    model_used: Option<String>,     // 实际使用的模型
    exit_code: i32,                 // 进程退出码
}

struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
}
```

### 5.3 Claude Code 调用方案

#### 基础调用

```bash
claude -p "{prompt}" \
  --output-format stream-json \
  --verbose \
  --model claude-sonnet-4-20250514 \
  --allowedTools "Edit,Write,Read,Glob,Grep" \
  --dangerously-skip-permissions

# 工作目录: {worktree_path}
# 环境变量: ANTHROPIC_API_KEY={decrypted_api_key}
```

#### 关键参数说明

| 参数 | 作用 |
|------|------|
| `--output-format stream-json` | JSONL 流式输出，每行一个 JSON 事件，支持实时日志展示 |
| `--verbose` | 输出详细事件（含 Token 统计等元信息） |
| `--allowedTools` | 工具白名单，限制 Agent 可用的操作 |
| `--dangerously-skip-permissions` | 跳过交互式审批（已通过 allowedTools 限制范围） |
| `--json-schema` | （可选）指定 JSON Schema 强制 Agent 输出结构化结果 |

#### 会话恢复（用于反馈循环）

```bash
# 首次执行，获取 session_id
claude -p "{prompt}" --output-format json | jq -r '.session_id'

# 基于反馈重新执行（恢复上下文）
claude -p "{feedback_prompt}" --resume "{session_id}" --output-format stream-json
```

### 5.4 Codex 调用方案

#### 基础调用

```bash
codex exec --json \
  --full-auto \
  --sandbox workspace-write \
  "{prompt}"

# 工作目录: {worktree_path}
# 环境变量: CODEX_API_KEY={decrypted_api_key}
```

#### 关键参数说明

| 参数 | 作用 |
|------|------|
| `--json` | JSONL 流式输出到 stdout，每行一个事件 |
| `--full-auto` | 允许自动执行编辑操作 |
| `--sandbox workspace-write` | 沙盒模式：允许工作目录写入 |
| `--output-schema` | （可选）指定 JSON Schema 强制最终输出结构化 |
| `-o {file}` | 将最终消息写入文件 |

#### 会话恢复

```bash
# 基于上次执行的反馈重新执行
codex exec resume --last "{feedback_prompt}"
# 或指定 session_id
codex exec resume {session_id} "{feedback_prompt}"
```

### 5.5 输出解析策略（核心优化）

**核心方案：JSONL 事件流解析**

两个 CLI 都支持 JSONL（JSON Lines）流式输出，每行一个 JSON 事件。这比等执行完再解析文本**有三大优势**：

1. **实时日志展示**：事件到达即可推送到前端，无需等待执行结束
2. **结构化数据**：Token 统计、文件变更等信息直接在事件中，无需正则解析
3. **错误即时感知**：失败事件实时捕获，可立即终止并通知用户

#### Claude Code JSONL 事件结构

```jsonl
{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Analyzing..."}}}
{"type":"result","result":"...","session_id":"abc123","cost":{"total_cost_usd":0.05}}
```

| 事件类型 | 用途 | 关键字段 |
|---------|------|---------|
| `stream_event` (text_delta) | 实时文本输出 → 前端日志面板 | `event.delta.text` |
| `result` | 最终结果 | `result`, `session_id`, `cost.total_cost_usd` |

#### Codex JSONL 事件结构

```jsonl
{"type":"thread.started","thread_id":"0199a213-..."}
{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"ls","status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"..."}}
{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122}}
```

| 事件类型 | 用途 | 关键字段 |
|---------|------|---------|
| `item.started/completed` | 实时操作日志 → 前端日志面板 | `item.type`, `item.text/command` |
| `turn.completed` | Token 统计 | `usage.input_tokens`, `usage.output_tokens` |
| `turn.failed` | 执行失败 | 错误信息 |

> **注意**：Codex 文档与实际输出存在字段名差异（文档 `item_type: "assistant_message"` vs 实际 `type: "agent_message"`）。解析器需同时兼容两种格式。

#### Rust 侧解析伪代码

```rust
// 逐行读取 JSONL 流
while let Some(line) = stdout_reader.next_line().await? {
    let event: serde_json::Value = serde_json::from_str(&line)?;
    
    match event["type"].as_str() {
        // 实时推送日志到前端
        Some("stream_event") | Some("item.started") | Some("item.completed") => {
            app_handle.emit("agent:log:{task_id}", &event)?;
        }
        // 提取 Token 统计
        Some("turn.completed") => {
            cost_tracker.update_from_event(&event);
        }
        // 最终结果
        Some("result") => {
            execution.result = event["result"].as_str().map(String::from);
            execution.session_id = event["session_id"].as_str().map(String::from);
            execution.cost = parse_cost(&event);
        }
        _ => {}
    }
}
```

#### 结构化输出（可选增强）

两个 CLI 都支持通过 JSON Schema 约束 Agent 的最终输出格式。当需要 Agent 返回特定结构的数据时使用：

```bash
# Claude Code
claude -p "{prompt}" --output-format json \
  --json-schema '{"type":"object","properties":{"summary":{"type":"string"},"risk_level":{"type":"string"}}}'

# Codex
codex exec --output-schema ./schema.json "{prompt}"
```

适用场景：代码审查报告、依赖分析结果、项目元数据提取等需要结构化数据的任务。

#### 文件变更检测

文件变更不依赖 Agent 输出解析，而是通过 Git Worktree 中的 `git diff` 获取（更可靠）：

```bash
cd {worktree_path}
git diff HEAD --name-status   # 变更文件列表
git diff HEAD                 # 完整 Diff 内容
git diff HEAD --stat          # 变更统计
```

这样**解析职责分离**：
- JSONL 事件流 → 实时日志 + Token 统计 + 执行状态
- Git Diff → 文件变更内容（最准确，不依赖 Agent 输出格式）

## 6. 健康检查

### 6.1 检查项

| 检查项 | 方法 | 频率 |
|--------|------|------|
| CLI 可用性 | 执行 `claude --version` / `codex --version` | 应用启动时 + 配置变更时 |
| API Key 格式 | 校验 Key 格式（如 `sk-ant-` 前缀） | 配置保存时 |
| CLI 版本兼容性 | 比对版本号是否满足最低要求 | 应用启动时 |
| 首次执行验证 | 首个 Agent 任务执行时验证完整链路 | 首次使用时 |

> 不做周期性网络探测和 API Key 实际验证，避免无意义的 Token 消耗和网络请求。API Key 的有效性在首次实际执行时自然验证。

### 6.2 状态指示

| 状态 | 图标 | 含义 |
|------|------|------|
| 在线可用 | 绿色圆点 | CLI 存在且版本兼容 |
| 版本过低 | 黄色圆点 | CLI 存在但版本不满足最低要求，提示升级 |
| 不可用 | 红色圆点 | CLI 未找到或路径无效 |
| 已禁用 | 灰色圆点 | 用户手动禁用 |

### 6.3 版本不兼容处理

```
应用启动
  ↓
检测已配置 Agent 的 CLI 版本
  ↓
版本低于最低要求？
  ├── 否 → 标记为在线可用
  └── 是 → 标记为"版本过低"
         ↓
       弹出提示通知：
       "Agent '{name}' 的 CLI 版本 ({current}) 低于最低要求 ({minimum})，
        部分功能可能不可用。请升级到最新版本。"
         ↓
       提供升级链接/命令
```

## 7. 数据库扩展

### 7.1 新增表：agent_configs

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PRIMARY KEY | 自增主键 |
| name | TEXT NOT NULL | Agent 显示名称 |
| agent_type | TEXT NOT NULL | 类型：claude_code / codex / custom |
| cli_path | TEXT | CLI 可执行文件路径 |
| cli_version | TEXT | 上次检测到的 CLI 版本号 |
| min_cli_version | TEXT | 最低兼容的 CLI 版本号 |
| api_key_encrypted | TEXT | AES-256-GCM 加密的 API Key |
| default_model | TEXT | 默认模型名称 |
| max_concurrent | INTEGER DEFAULT 1 | 最大并发数 |
| timeout_seconds | INTEGER DEFAULT 300 | 默认超时（秒） |
| capabilities | TEXT | JSON 格式：`{"code_gen":8,"code_fix":7,...}` |
| env_vars | TEXT | JSON 格式的环境变量 |
| sandbox_config | TEXT | JSON 格式的沙盒配置 |
| enabled | INTEGER DEFAULT 1 | 是否启用 |
| created_at | TEXT | 创建时间 |
| updated_at | TEXT | 更新时间 |

## 8. 沙盒隔离方案

Agent 执行时需要一定程度的隔离，防止误操作影响用户的工作环境。采用 **Git Worktree 隔离 + 路径白名单** 的方案。

### 8.1 为什么选 Git Worktree 而非 Git Branch 切换

| 方案 | 优点 | 缺点 |
|------|------|------|
| ~~Git Branch 切换~~ | 简单直接 | **会打断用户工作**：需要 stash 未提交修改、切换分支，用户无法同时编辑代码 |
| **Git Worktree** | **不影响用户当前工作**：在独立目录中创建分支的工作副本 | 占用额外磁盘空间 |

### 8.2 Git Worktree 隔离流程

```
Agent 任务准备执行
  ↓
1. 检查 project_path 是否为有效 Git 仓库
  ↓
2. 创建 Worktree：
   git worktree add {worktree_path} -b agent/{task_id} HEAD
   // worktree_path = {project_path}/.agent-worktrees/{task_id}
  ↓
3. Agent 在 worktree_path 中执行（用户的主工作目录不受影响）
  ↓
4. 执行完成后，在 worktree 中收集 git diff
  ↓
5. 审核通过 → 将 agent/{task_id} 分支合并回用户的当前分支
   审核拒绝 → 清理 worktree 和分支
  ↓
6. 清理 Worktree：
   git worktree remove {worktree_path}
   git branch -d agent/{task_id}  // 合并后删除
```

### 8.3 隔离层级

| 层级 | 机制 | 说明 |
|------|------|------|
| **工作目录隔离** | Git Worktree | Agent 在独立工作副本中操作，不影响用户的主工作目录 |
| **路径白名单** | CLI 参数 | 仅允许 Agent 在 worktree 目录下操作 |
| **工具白名单** | `--allowedTools` | 限制 Agent 可使用的工具（如禁止执行 Shell 命令） |
| **只读保护** | 文件过滤 | 配置关键文件（.env, CI 配置等）不被 Agent 修改 |

### 8.4 Agent CLI 隔离参数

| Agent | 隔离参数 | 说明 |
|-------|---------|------|
| Claude Code | `--allowedTools "Edit,Write,Read,Glob,Grep"` | 仅允许文件操作，禁止 Bash 等高风险工具 |
| Claude Code | `--dangerously-skip-permissions` | 跳过交互式审批（已通过 allowedTools 限制范围） |
| Codex | 沙盒模式参数 | 根据 Codex CLI 提供的隔离选项配置 |

### 8.5 保护规则配置

```
SandboxConfig {
    enable_worktree_isolation: boolean  // 是否启用 Worktree 隔离（默认 true）
    protected_files: string[]           // 受保护文件列表（如 .env, .gitignore）
    allowed_tools: string[]             // Agent 可使用的工具白名单
    max_files_changed: number           // 单次最多允许修改的文件数（超出需人工确认）
    max_lines_changed: number           // 单次最多允许修改的行数
    worktree_base_dir: string           // Worktree 存放目录（默认 .agent-worktrees）
}
```

## 9. 已确定的设计决策

| 问题 | 决策 | 说明 |
|------|------|------|
| 远程 Agent（HTTP API） | **暂不支持** | 当前仅支持本地 CLI 调用，后续根据需求再扩展 |
| 沙盒隔离 | **Git Worktree + 工具白名单** | 不打断用户工作，在独立目录中执行 |
| CLI 版本不兼容 | **检测并提示升级** | 启动时检查版本，不兼容时弹出提示并提供升级链接 |
| API Key 存储 | **AES-256-GCM 加密存 SQLite** | 密钥派生自机器唯一标识，安全且便于管理 |
| API Key 多 Key 轮换 | **不支持** | 单 Key 即可满足当前需求 |
| Agent 扩展方式 | **CLI 适配器模式** | 通过统一的 `AgentRunner` Trait 接入新 Agent（opencode、aider 等） |
| 能力画像维度 | **MVP 5 维度 + 1-10 量化评分** | 精简够用，支持智能分配算法 |
| 健康检查频率 | **仅启动时和配置变更时** | 避免无意义的周期性网络请求和 Token 消耗 |
