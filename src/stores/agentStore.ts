import { defineStore } from 'pinia'
import { ref, computed, reactive } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AgentConfig,
  CreateAgentRequest,
  UpdateAgentRequest,
  AgentHealthStatus,
  AgentEvent,
} from '@/types/agent'

export interface ExecutionInfo {
  taskId: string
  subtaskId: number
  agentId: number
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  logs: Array<{ content: string; level: string; timestampMs: number }>
  startTimeMs: number
  durationMs?: number
  error?: string
}

export interface ExecutionState {
  taskId: string
  status: string
  logs: Array<{ content: string; level: string; timestampMs: number }>
  result?: {
    textResponse: string
    inputTokens?: number
    outputTokens?: number
    exitCode: number
    durationMs: number
  }
  error?: string
  startTimeMs: number
  durationMs?: number
}

export const useAgentStore = defineStore('agent', () => {
  const agents = ref<AgentConfig[]>([])
  const healthStatuses = ref<Map<number, AgentHealthStatus>>(new Map())
  const loading = ref(false)

  const activeExecutions = reactive<Map<number, ExecutionInfo>>(new Map())
  const eventListeners = new Map<string, UnlistenFn>()

  const enabledAgents = computed(() => agents.value.filter(a => a.enabled))
  const agentCount = computed(() => agents.value.length)

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

  async function addAgent(request: CreateAgentRequest): Promise<number> {
    const id = await invoke<number>('create_agent', { request })
    await loadAgents()
    return id
  }

  async function editAgent(id: number, request: UpdateAgentRequest) {
    await invoke('update_agent', { id, request })
    await loadAgents()
  }

  async function removeAgent(id: number) {
    await invoke('delete_agent', { id })
    await loadAgents()
  }

  async function checkHealth(id: number): Promise<AgentHealthStatus> {
    const status = await invoke<AgentHealthStatus>('check_agent_health', { id })
    healthStatuses.value.set(id, status)
    return status
  }

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

  function getHealthStatus(id: number): AgentHealthStatus | undefined {
    return healthStatuses.value.get(id)
  }

  function getExecutionForSubtask(subtaskId: number): ExecutionInfo | undefined {
    return activeExecutions.get(subtaskId)
  }

  async function startBackgroundExecution(
    agentId: number,
    prompt: string,
    projectPath: string,
    taskId: string,
    subtaskId: number,
  ): Promise<void> {
    const info: ExecutionInfo = {
      taskId,
      subtaskId,
      agentId,
      status: 'running',
      logs: [],
      startTimeMs: Date.now(),
    }
    activeExecutions.set(subtaskId, info)

    const unlisten = await listen<AgentEvent>(`agent:log:${taskId}`, (event) => {
      const exec = activeExecutions.get(subtaskId)
      if (!exec) return

      const payload = event.payload
      switch (payload.kind) {
        case 'Log':
          exec.logs.push({
            content: payload.content || '',
            level: payload.level || 'stdout',
            timestampMs: Date.now(),
          })
          break
        case 'TokenUsage':
          break
        case 'Completed':
          exec.status = 'completed'
          exec.durationMs = Date.now() - exec.startTimeMs
          cleanupListener(taskId)
          break
        case 'Failed':
          exec.status = 'failed'
          exec.error = payload.error
          exec.durationMs = Date.now() - exec.startTimeMs
          cleanupListener(taskId)
          break
      }
    })
    eventListeners.set(taskId, unlisten)

    await invoke('start_agent_execution', {
      agentId,
      prompt,
      projectPath,
      taskId,
      subtaskId,
    })
  }

  function cleanupListener(taskId: string) {
    const unlisten = eventListeners.get(taskId)
    if (unlisten) {
      unlisten()
      eventListeners.delete(taskId)
    }
  }

  async function cancelExecution(taskId: string) {
    await invoke('cancel_agent_execution', { taskId })
    cleanupListener(taskId)
  }

  async function fetchExecutionState(taskId: string): Promise<ExecutionState | null> {
    return await invoke<ExecutionState | null>('get_agent_execution_state', { taskId })
  }

  async function restoreExecutionForSubtask(subtaskId: number): Promise<ExecutionInfo | null> {
    const existing = activeExecutions.get(subtaskId)
    if (existing) return existing

    const state = await invoke<ExecutionState | null>('get_agent_execution_by_subtask', { subtaskId })
    if (!state) return null

    const info: ExecutionInfo = {
      taskId: state.taskId,
      subtaskId,
      agentId: 0,
      status: state.status as ExecutionInfo['status'],
      logs: state.logs || [],
      startTimeMs: state.startTimeMs,
      durationMs: state.durationMs,
      error: state.error,
    }
    activeExecutions.set(subtaskId, info)

    if (state.status === 'running') {
      const unlisten = await listen<AgentEvent>(`agent:log:${state.taskId}`, (event) => {
        const exec = activeExecutions.get(subtaskId)
        if (!exec) return
        const payload = event.payload
        switch (payload.kind) {
          case 'Log':
            exec.logs.push({
              content: payload.content || '',
              level: payload.level || 'stdout',
              timestampMs: Date.now(),
            })
            break
          case 'Completed':
            exec.status = 'completed'
            exec.durationMs = Date.now() - exec.startTimeMs
            cleanupListener(state.taskId)
            break
          case 'Failed':
            exec.status = 'failed'
            exec.error = payload.error
            exec.durationMs = Date.now() - exec.startTimeMs
            cleanupListener(state.taskId)
            break
        }
      })
      eventListeners.set(state.taskId, unlisten)
    }

    return info
  }

  function removeExecution(subtaskId: number) {
    const exec = activeExecutions.get(subtaskId)
    if (exec) {
      cleanupListener(exec.taskId)
      activeExecutions.delete(subtaskId)
    }
  }

  return {
    agents,
    healthStatuses,
    loading,
    enabledAgents,
    agentCount,
    activeExecutions,
    loadAgents,
    addAgent,
    editAgent,
    removeAgent,
    checkHealth,
    checkAllHealth,
    getHealthStatus,
    getExecutionForSubtask,
    startBackgroundExecution,
    cancelExecution,
    fetchExecutionState,
    restoreExecutionForSubtask,
    removeExecution,
  }
})
