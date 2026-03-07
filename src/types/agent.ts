export type AgentType = 'claude_code' | 'codex' | 'custom'

export type AgentHealthState = 'healthy' | 'outdated' | 'unavailable' | 'no_key' | 'error' | 'disabled'


export interface AgentConfig {
  id: number
  name: string
  agentType: AgentType
  cliPath: string
  enabled: boolean
  createdAt: string
  updatedAt: string
}

export interface CreateAgentRequest {
  name: string
  agentType: AgentType
  cliPath: string
}

export interface UpdateAgentRequest {
  name?: string
  agentType?: string
  cliPath?: string
  enabled?: boolean
}

export interface AgentHealthStatus {
  agentId: number
  status: AgentHealthState
  cliFound: boolean
  detectedVersion: string | null
  versionCompatible: boolean
  message: string | null
}

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

