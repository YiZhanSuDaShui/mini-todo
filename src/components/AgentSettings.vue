<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { ElMessage } from 'element-plus'
import { Refresh, Connection } from '@element-plus/icons-vue'
import AgentStatusBadge from './AgentStatusBadge.vue'
import { useAgentStore } from '@/stores/agentStore'
import { AGENT_TYPE_INFO } from '@/types/agent'
import type { AgentHealthState } from '@/types/agent'

interface DetectedAgent {
  agentType: string
  cliPath: string
  version: string | null
  available: boolean
}

const agentStore = useAgentStore()
const detecting = ref(false)
const detectedAgents = ref<DetectedAgent[]>([])
const checkingHealthId = ref<number | null>(null)

function getHealthState(agentId: number): AgentHealthState {
  const status = agentStore.getHealthStatus(agentId)
  return (status?.status as AgentHealthState) || 'unavailable'
}

function getHealthMessage(agentId: number): string {
  const status = agentStore.getHealthStatus(agentId)
  return status?.message || ''
}

async function detectAgents() {
  detecting.value = true
  try {
    detectedAgents.value = await invoke<DetectedAgent[]>('auto_detect_agents')
    await agentStore.loadAgents()
    if (agentStore.agents.length > 0) {
      await agentStore.checkAllHealth()
    }
    const found = detectedAgents.value.filter(a => a.available).length
    ElMessage.success(`检测完成：发现 ${found} 个可用 Agent`)
  } catch (e) {
    ElMessage.error('检测失败: ' + String(e))
  } finally {
    detecting.value = false
  }
}

async function handleCheckHealth(agentId: number) {
  checkingHealthId.value = agentId
  try {
    const result = await agentStore.checkHealth(agentId)
    ElMessage.info(result.message || '检测完成')
  } catch (e) {
    ElMessage.error('检测失败: ' + String(e))
  } finally {
    checkingHealthId.value = null
  }
}

onMounted(async () => {
  await agentStore.loadAgents()
  if (agentStore.agents.length === 0) {
    await detectAgents()
  } else {
    await agentStore.checkAllHealth()
  }
})
</script>

<template>
  <div class="agent-settings">
    <div class="settings-desc">
      <p>自动检测系统中安装的 Agent CLI 工具。CLI 工具需自行安装并配置好 API Key。</p>
    </div>

    <div class="agent-list">
      <div
        v-for="agent in agentStore.agents"
        :key="agent.id"
        class="agent-card"
      >
        <div class="agent-header">
          <div class="agent-name">
            <span>{{ agent.name }}</span>
            <el-tag size="small" type="info" class="agent-type-tag">
              {{ AGENT_TYPE_INFO[agent.agentType]?.label || agent.agentType }}
            </el-tag>
          </div>
          <AgentStatusBadge :status="getHealthState(agent.id)" />
        </div>

        <div class="agent-info">
          <span class="info-item">
            {{ agent.cliPath }}
          </span>
          <span v-if="getHealthMessage(agent.id)" class="info-item">
            {{ getHealthMessage(agent.id) }}
          </span>
        </div>

        <div class="agent-actions">
          <el-switch
            :model-value="agent.enabled"
            size="small"
            @change="agentStore.editAgent(agent.id, { enabled: !agent.enabled })"
          />
          <el-button
            size="small"
            :icon="Connection"
            :loading="checkingHealthId === agent.id"
            @click="handleCheckHealth(agent.id)"
          >
            检测
          </el-button>
        </div>
      </div>

      <div v-if="agentStore.agents.length === 0 && !detecting" class="empty-state">
        <p>未检测到 Agent</p>
        <p class="empty-hint">请先安装 Claude Code 或 Codex CLI</p>
      </div>
    </div>

    <el-button
      type="primary"
      :icon="Refresh"
      :loading="detecting"
      style="width: 100%; margin-top: 12px"
      @click="detectAgents"
    >
      {{ detecting ? '检测中...' : '重新检测' }}
    </el-button>
  </div>
</template>

<style scoped>
.agent-settings {
  padding: 0;
}

.settings-desc {
  margin-bottom: 12px;

  p {
    font-size: 12px;
    color: var(--text-tertiary);
    margin: 0;
    line-height: 1.5;
  }
}

.agent-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.agent-card {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 12px;
  transition: border-color var(--transition-fast);
}

.agent-card:hover {
  border-color: var(--primary-light);
}

.agent-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}

.agent-name {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}

.agent-type-tag {
  font-weight: 400;
}

.agent-info {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 10px;
}

.info-item {
  font-size: 12px;
  color: var(--text-tertiary);
}

.agent-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.empty-state {
  text-align: center;
  padding: 24px 0;
  color: var(--text-secondary);
  font-size: 14px;
}

.empty-hint {
  font-size: 12px;
  color: var(--text-tertiary);
  margin-top: 4px;
}
</style>
