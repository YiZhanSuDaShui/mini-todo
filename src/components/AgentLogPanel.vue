<script setup lang="ts">
import { ref, nextTick, onBeforeUnmount, onMounted, computed } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { marked } from 'marked'
import type { AgentEvent } from '@/types/agent'

const props = defineProps<{
  taskId: string
  initialStatus?: 'idle' | 'running' | 'completed' | 'failed'
  initialLogs?: Array<{ content: string; level: string }>
  initialStartTime?: number
}>()

interface LogLine {
  content: string
  level: string
  timestamp: number
}

const logLines = ref<LogLine[]>([])
const containerRef = ref<HTMLElement | null>(null)
const autoScroll = ref(true)
const inputTokens = ref(0)
const outputTokens = ref(0)
const status = ref<'idle' | 'running' | 'completed' | 'failed'>('idle')
const startTime = ref(0)
const elapsed = ref(0)
let timer: ReturnType<typeof setInterval> | null = null
let unlisten: UnlistenFn | null = null

function appendLog(content: string, level: string = 'stdout') {
  logLines.value.push({ content, level, timestamp: Date.now() })
  if (autoScroll.value) {
    nextTick(() => {
      if (containerRef.value) {
        containerRef.value.scrollTop = containerRef.value.scrollHeight
      }
    })
  }
}

function handleEvent(event: AgentEvent) {
  switch (event.kind) {
    case 'Log':
      appendLog(event.content || '', event.level || 'stdout')
      break
    case 'Progress':
      appendLog(event.message || '', 'info')
      break
    case 'TokenUsage':
      inputTokens.value += event.inputTokens || 0
      outputTokens.value += event.outputTokens || 0
      break
    case 'Completed':
      status.value = 'completed'
      stopTimer()
      appendLog(`[完成] exit_code=${event.exitCode}`, 'success')
      break
    case 'Failed':
      status.value = 'failed'
      stopTimer()
      appendLog(`[失败] ${event.error}`, 'stderr')
      break
  }
}

function startTimer(fromTime: number) {
  startTime.value = fromTime
  elapsed.value = Math.floor((Date.now() - fromTime) / 1000)
  stopTimer()
  timer = setInterval(() => {
    elapsed.value = Math.floor((Date.now() - startTime.value) / 1000)
  }, 1000)
}

function stopTimer() {
  if (timer) {
    clearInterval(timer)
    timer = null
  }
}

const renderedMarkdown = computed(() => {
  if (logLines.value.length === 0) return ''
  const deduped = deduplicateLogs(logLines.value)
  const parts = deduped.map(line => {
    if (line.level === 'stderr') return `<span class="log-stderr">${escapeHtml(line.content)}</span>`
    if (line.level === 'success') return `<span class="log-success">${escapeHtml(line.content)}</span>`
    return formatCommandLines(line.content)
  })
  const md = mergeAdjacentCodeBlocks(parts.join('\n\n'))
  return marked.parse(md, { breaks: true, async: false }) as string
})

function deduplicateLogs(lines: LogLine[]): LogLine[] {
  const result: LogLine[] = []
  for (let i = 0; i < lines.length; i++) {
    if (i > 0
      && lines[i].content === lines[i - 1].content
      && lines[i].level === lines[i - 1].level
    ) {
      continue
    }
    result.push(lines[i])
  }
  return result
}

function mergeAdjacentCodeBlocks(text: string): string {
  return text.replace(/```\s*```(\w*\n)?/g, '\n')
}

function formatCommandLines(text: string): string {
  if (text.includes('```')) return text

  const lines = text.split('\n')
  const result: string[] = []
  let cmdBlock: string[] = []
  let patchBlock: string[] = []
  let inPatch = false

  for (const line of lines) {
    const trimmed = line.trim()

    if (trimmed.startsWith('*** Begin Patch') || trimmed.startsWith('*** begin patch')) {
      if (cmdBlock.length > 0) {
        result.push('```\n' + cmdBlock.join('\n') + '\n```')
        cmdBlock = []
      }
      inPatch = true
      patchBlock = [line]
      continue
    }

    if (inPatch) {
      patchBlock.push(line)
      if (trimmed.startsWith('*** End Patch') || trimmed.startsWith('*** end patch')) {
        inPatch = false
        continue
      }
      continue
    }

    if (patchBlock.length > 0) {
      if (trimmed === "'@\"" || trimmed === "'@" || trimmed === '"@' || trimmed === "@'" || trimmed === '') {
        patchBlock.push(line)
        if (trimmed !== '') {
          result.push('```\n' + patchBlock.join('\n') + '\n```')
          patchBlock = []
        }
        continue
      } else {
        result.push('```\n' + patchBlock.join('\n') + '\n```')
        patchBlock = []
      }
    }

    if (trimmed.startsWith('$ ')) {
      cmdBlock.push(trimmed)
    } else {
      if (cmdBlock.length > 0) {
        result.push('```\n' + cmdBlock.join('\n') + '\n```')
        cmdBlock = []
      }
      result.push(line)
    }
  }

  if (cmdBlock.length > 0) {
    result.push('```\n' + cmdBlock.join('\n') + '\n```')
  }
  if (patchBlock.length > 0) {
    result.push('```diff\n' + patchBlock.join('\n') + '\n```')
  }

  return result.join('\n')
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

function handleScroll() {
  if (!containerRef.value) return
  const { scrollTop, scrollHeight, clientHeight } = containerRef.value
  autoScroll.value = scrollHeight - scrollTop - clientHeight < 30
}

function formatElapsed(secs: number): string {
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return m > 0 ? `${m}m ${s}s` : `${s}s`
}

onMounted(async () => {
  if (props.initialLogs?.length) {
    for (const log of props.initialLogs) {
      logLines.value.push({ content: log.content, level: log.level, timestamp: Date.now() })
    }
  }

  status.value = props.initialStatus || 'idle'

  if (status.value === 'running') {
    startTimer(props.initialStartTime || Date.now())
    unlisten = await listen<AgentEvent>(`agent:log:${props.taskId}`, (event) => {
      handleEvent(event.payload)
    })
  }
})

onBeforeUnmount(() => {
  stopTimer()
  if (unlisten) {
    unlisten()
    unlisten = null
  }
})
</script>

<template>
  <div class="agent-log-panel">
    <div
      ref="containerRef"
      class="log-container"
      @scroll="handleScroll"
    >
      <div v-if="logLines.length === 0" class="log-empty">
        等待执行...
      </div>
      <div
        v-else
        class="log-markdown"
        v-html="renderedMarkdown"
      ></div>
    </div>
    <div class="status-bar">
      <span v-if="status === 'running'">
        运行中 {{ formatElapsed(elapsed) }}
      </span>
      <span v-else-if="status === 'completed'" class="status-success">
        已完成 {{ formatElapsed(elapsed) }}
      </span>
      <span v-else-if="status === 'failed'" class="status-error">
        已失败
      </span>
      <span v-else>就绪</span>
      <span v-if="inputTokens > 0 || outputTokens > 0">
        Token: {{ inputTokens }} in / {{ outputTokens }} out
      </span>
    </div>
  </div>
</template>

<style scoped>
.agent-log-panel {
  border-radius: var(--radius-base);
  overflow: hidden;
  border: 1px solid var(--border);
}

.log-container {
  background-color: #1e1e1e;
  color: #d4d4d4;
  font-family: 'Consolas', 'Source Code Pro', 'Courier New', monospace;
  font-size: 13px;
  line-height: 1.5;
  padding: 12px;
  min-height: 120px;
}

.log-empty {
  color: #858585;
  font-style: italic;
}

.log-markdown {
  word-break: break-word;
}

.log-markdown :deep(p) {
  margin: 4px 0;
  line-height: 1.6;
}

.log-markdown :deep(pre) {
  background: #2d2d2d;
  border: 1px solid #404040;
  border-radius: 4px;
  padding: 8px 12px;
  margin: 6px 0;
  font-size: 12px;
  white-space: pre-wrap;
  word-break: break-all;
}

.log-markdown :deep(code) {
  background: #2d2d2d;
  padding: 1px 5px;
  border-radius: 3px;
  font-size: 12px;
  color: #e6db74;
}

.log-markdown :deep(pre code) {
  padding: 0;
  background: transparent;
  color: #d4d4d4;
}

.log-markdown :deep(strong) {
  color: #6796e6;
}

.log-markdown :deep(.log-stderr) {
  color: #f48771;
}

.log-markdown :deep(.log-success) {
  color: #89d185;
}

.log-markdown :deep(hr) {
  border: none;
  border-top: 1px solid #404040;
  margin: 8px 0;
}

.log-markdown :deep(h1),
.log-markdown :deep(h2),
.log-markdown :deep(h3) {
  color: #6796e6;
  margin: 8px 0 4px;
  font-size: 14px;
}

.status-bar {
  display: flex;
  justify-content: space-between;
  padding: 6px 12px;
  background: #252526;
  font-size: 12px;
  color: #858585;
  font-family: 'Consolas', 'Source Code Pro', 'Courier New', monospace;
}

.status-success {
  color: #89d185;
}

.status-error {
  color: #f48771;
}
</style>
