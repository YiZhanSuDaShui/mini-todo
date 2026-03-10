# Agent 集成层 - Vue 前端实现

## 1. TypeScript 类型定义 (types/agent.ts)

```typescript
// Agent 类型枚举
export type AgentType = 'claude_code' | 'codex' | 'custom'

// Agent 健康状态
export type AgentHealthState = 'online' | 'outdated' | 'unavailable' | 'disabled'

// Agent 能力画像
export interface AgentCapabilities {
  codeGen: number       // 代码生成 1-10
  codeFix: number       // 代码修复 1-10
  testReview: number    // 测试与审查 1-10
  speed: number         // 速度 1-10
  costEfficiency: number // 成本效率 1-10
}

// 沙盒配置
export interface SandboxConfig {
  enableWorktreeIsolation: boolean
  protectedFiles: string[]
  allowedTools: string[]
  maxFilesChanged: number
  maxLinesChanged: number
  worktreeBaseDir: string
}

// Agent 配置
export interface AgentConfig {
  id: number
  name: string
  agentType: AgentType
  cliPath: string
  cliVersion: string
  minCliVersion: string
  defaultModel: string
  maxConcurrent: number
  timeoutSeconds: number
  capabilities: string    // JSON 字符串
  envVars: string         // JSON 字符串
  sandboxConfig: string   // JSON 字符串
  enabled: boolean
  hasApiKey: boolean
  createdAt: string
  updatedAt: string
}

// 创建 Agent 请求
export interface CreateAgentRequest {
  name: string
  agentType: AgentType
  cliPath: string
  apiKey?: string
  defaultModel?: string
  maxConcurrent?: number
  timeoutSeconds?: number
  capabilities?: string
  envVars?: string
  sandboxConfig?: string
}

// 更新 Agent 请求
export interface UpdateAgentRequest {
  name?: string
  agentType?: string
  cliPath?: string
  apiKey?: string
  defaultModel?: string
  maxConcurrent?: number
  timeoutSeconds?: number
  capabilities?: string
  envVars?: string
  sandboxConfig?: string
  enabled?: boolean
}

// Agent 健康状态
export interface AgentHealthStatus {
  agentId: number
  status: AgentHealthState
  cliFound: boolean
  detectedVersion: string | null
  versionCompatible: boolean
  message: string | null
}

// Agent 执行事件（从 Tauri listen 接收）
export interface AgentEvent {
  kind: 'Log' | 'Progress' | 'TokenUsage' | 'Completed' | 'Failed'
  content?: string
  level?: string
  message?: string
  inputTokens?: number
  outputTokens?: number
  exitCode?: number
  result?: string
  error?: string
}

// Agent 类型信息（用于 UI 展示）
export const AGENT_TYPE_INFO: Record<AgentType, { label: string; description: string }> = {
  claude_code: {
    label: 'Claude Code',
    description: '复杂架构设计、多文件重构、深度代码理解',
  },
  codex: {
    label: 'Codex',
    description: '快速代码生成、小范围修改',
  },
  custom: {
    label: '自定义',
    description: '通过 CLI 适配器接入的其他工具',
  },
}

// 默认能力预设
export const DEFAULT_CAPABILITIES: Record<string, AgentCapabilities> = {
  claude_code: { codeGen: 8, codeFix: 8, testReview: 7, speed: 7, costEfficiency: 7 },
  codex: { codeGen: 6, codeFix: 6, testReview: 5, speed: 8, costEfficiency: 8 },
  custom: { codeGen: 5, codeFix: 5, testReview: 5, speed: 5, costEfficiency: 5 },
}

// 默认沙盒配置
export const DEFAULT_SANDBOX_CONFIG: SandboxConfig = {
  enableWorktreeIsolation: true,
  protectedFiles: ['.env', '.env.local', '.gitignore'],
  allowedTools: ['Edit', 'Write', 'Read', 'Glob', 'Grep'],
  maxFilesChanged: 50,
  maxLinesChanged: 5000,
  worktreeBaseDir: '.agent-worktrees',
}
```

## 2. Pinia Store (stores/agentStore.ts)

```typescript
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type {
  AgentConfig,
  CreateAgentRequest,
  UpdateAgentRequest,
  AgentHealthStatus,
  AgentEvent,
} from '@/types/agent'

export const useAgentStore = defineStore('agent', () => {
  // 状态
  const agents = ref<AgentConfig[]>([])
  const healthStatuses = ref<Map<number, AgentHealthStatus>>(new Map())
  const loading = ref(false)

  // 计算属性
  const enabledAgents = computed(() => agents.value.filter(a => a.enabled))
  const agentCount = computed(() => agents.value.length)

  // 加载所有 Agent 配置
  async function loadAgents() {
    loading.value = true
    try {
      agents.value = await invoke<AgentConfig[]>('get_agents')
    } catch (e) {
      console.error('加载 Agent 配置失败:', e)
    } finally {
      loading.value = false
    }
  }

  // 创建 Agent
  async function addAgent(request: CreateAgentRequest): Promise<number> {
    const id = await invoke<number>('create_agent', { request })
    await loadAgents()
    return id
  }

  // 更新 Agent
  async function editAgent(id: number, request: UpdateAgentRequest) {
    await invoke('update_agent', { id, request })
    await loadAgents()
  }

  // 删除 Agent
  async function removeAgent(id: number) {
    await invoke('delete_agent', { id })
    await loadAgents()
  }

  // 检查单个 Agent 健康状态
  async function checkHealth(id: number): Promise<AgentHealthStatus> {
    const status = await invoke<AgentHealthStatus>('check_agent_health', { id })
    healthStatuses.value.set(id, status)
    return status
  }

  // 检查所有 Agent 健康状态
  async function checkAllHealth() {
    try {
      const statuses = await invoke<AgentHealthStatus[]>('check_all_agents_health')
      healthStatuses.value.clear()
      for (const s of statuses) {
        healthStatuses.value.set(s.agentId, s)
      }
    } catch (e) {
      console.error('健康检查失败:', e)
    }
  }

  // 获取 Agent 的健康状态
  function getHealthStatus(id: number): AgentHealthStatus | undefined {
    return healthStatuses.value.get(id)
  }

  return {
    agents,
    healthStatuses,
    loading,
    enabledAgents,
    agentCount,
    loadAgents,
    addAgent,
    editAgent,
    removeAgent,
    checkHealth,
    checkAllHealth,
    getHealthStatus,
  }
})
```

## 3. 组件设计

### 3.1 AgentSettings.vue（Agent 管理页面）

在设置弹窗中新增 "Agent 管理" Tab，展示 Agent 列表和管理操作。

**组件结构**：

```
AgentSettings.vue
├── Agent 列表区域
│   ├── AgentCard × N（每个 Agent 一张卡片）
│   │   ├── 名称 + 类型标签
│   │   ├── AgentStatusBadge（状态指示器）
│   │   ├── 模型信息
│   │   ├── 能力标签
│   │   └── 操作按钮（编辑 / 删除 / 启用切换 / 测试连接）
│   └── "添加 Agent" 按钮
│
└── Agent 编辑弹窗（el-dialog）
    ├── 基本信息（名称、类型、CLI 路径）
    ├── API Key 输入（密码框）
    ├── 模型选择
    ├── 高级设置折叠面板
    │   ├── 超时时间
    │   ├── 并发数
    │   ├── 环境变量编辑
    │   └── 沙盒配置
    └── 能力画像（5 个滑块）
```

**关键交互**：

| 操作 | 行为 |
|------|------|
| 添加 Agent | 打开编辑弹窗，类型选择后自动填充默认值 |
| 编辑 | 打开编辑弹窗，加载现有配置 |
| 删除 | 二次确认后删除 |
| 启用/禁用 | Switch 切换，即时保存 |
| 测试连接 | 调用 check_agent_health，显示检测结果 |

### 3.2 AgentStatusBadge.vue（状态指示器）

小型组件，显示 Agent 的在线状态。

```vue
<template>
  <el-tag :type="tagType" size="small" round>
    <span class="status-dot" :style="{ backgroundColor: dotColor }" />
    {{ statusText }}
  </el-tag>
</template>
```

| 状态 | 圆点颜色 | 文字 | el-tag type |
|------|---------|------|------------|
| online | #10B981 | 在线 | success |
| outdated | #F59E0B | 版本过低 | warning |
| unavailable | #EF4444 | 不可用 | danger |
| disabled | #9CA3AF | 已禁用 | info |

### 3.3 AgentLogPanel.vue（执行日志面板）

终端风格的实时日志展示组件，用于展示 Agent 执行过程中的输出。

**功能**：
- 深色背景，等宽字体（Consolas / Source Code Pro）
- 自动滚动到底部（可手动暂停）
- 底部状态栏显示：运行时间、Token 使用量
- 支持清空日志
- 支持搜索

**接收事件**：

```typescript
import { listen } from '@tauri-apps/api/event'

// 监听 Agent 执行日志事件
const unlisten = await listen<AgentEvent>(`agent:log:${taskId}`, (event) => {
  const agentEvent = event.payload
  switch (agentEvent.kind) {
    case 'Log':
      appendLog(agentEvent.content!, agentEvent.level!)
      break
    case 'TokenUsage':
      updateTokenUsage(agentEvent.inputTokens!, agentEvent.outputTokens!)
      break
    case 'Completed':
      setCompleted(agentEvent.exitCode!, agentEvent.result!)
      break
    case 'Failed':
      setFailed(agentEvent.error!)
      break
  }
})
```

**样式参考**：

```css
.agent-log-panel {
  background-color: #1e1e1e;
  color: #d4d4d4;
  font-family: 'Consolas', 'Source Code Pro', 'Courier New', monospace;
  font-size: 13px;
  line-height: 1.5;
  padding: 12px;
  border-radius: 6px;
  overflow-y: auto;
  max-height: 400px;
}

.log-line {
  white-space: pre-wrap;
  word-break: break-all;
}

.log-line--stderr {
  color: #f48771;
}

.status-bar {
  display: flex;
  justify-content: space-between;
  padding: 8px 12px;
  background: #252526;
  border-top: 1px solid #333;
  font-size: 12px;
  color: #858585;
}
```

## 4. 设置页面集成

现有的设置弹窗需要新增 "Agent" Tab。查看现有设置组件的 Tab 结构，在其中追加：

```vue
<el-tab-pane label="Agent 管理" name="agent">
  <AgentSettings />
</el-tab-pane>
```

## 5. 文件修改清单

### 新增文件

| 文件 | 说明 |
|------|------|
| `src/types/agent.ts` | Agent TypeScript 类型和常量 |
| `src/stores/agentStore.ts` | Agent Pinia Store |
| `src/components/AgentSettings.vue` | Agent 配置管理界面 |
| `src/components/AgentStatusBadge.vue` | 状态指示器组件 |
| `src/components/AgentLogPanel.vue` | 执行日志面板 |

### 修改文件

| 文件 | 变更 |
|------|------|
| `src/types/index.ts` | 导出 agent 类型 |
| `src/stores/index.ts` | 导出 agentStore |
| 设置弹窗组件 | 新增 "Agent 管理" Tab |

## 6. UI 规范遵循

根据 CLAUDE.md 的要求：

- 使用 Element Plus 组件库
- 图标使用 @element-plus/icons-vue
- 不使用 emoji
- 设计风格：简洁现代、极简
- 配色遵循现有项目风格
