<script setup lang="ts">
import { computed } from 'vue'
import { useTodoStore } from '@/stores'
import { ElMessageBox } from 'element-plus'
import dayjs from 'dayjs'
import type { Todo } from '@/types'

const props = defineProps<{
  todo: Todo
}>()

const emit = defineEmits<{
  (e: 'click'): void
  (e: 'toggle-complete'): void
  (e: 'delete'): void
}>()

const todoStore = useTodoStore()

// 是否已完成
const isCompleted = computed(() => props.todo.completed)

// 颜色样式
const colorStyle = computed(() => ({
  backgroundColor: props.todo.color
}))

// 子任务统计
const subtaskStats = computed(() => {
  const total = props.todo.subtasks.length
  const completed = props.todo.subtasks.filter(s => s.completed).length
  return { total, completed }
})

// 格式化通知时间
const formattedNotifyTime = computed(() => {
  const firstReminder = props.todo.reminderTimes?.[0]
  if (!firstReminder) return null
  return dayjs(firstReminder).format('MM-DD HH:mm')
})

// 切换完成状态
function toggleComplete(e: Event) {
  e.stopPropagation()
  emit('toggle-complete')
}

// 删除待办
async function deleteTodo(e: Event) {
  e.stopPropagation()
  try {
    await ElMessageBox.confirm(
      `确定要删除待办"${props.todo.title}"吗？`,
      '删除确认',
      {
        confirmButtonText: '删除',
        cancelButtonText: '取消',
        type: 'warning'
      }
    )
    emit('delete')
  } catch {
    // 用户取消，不做任何操作
  }
}

// 置顶/取消置顶待办
async function togglePin(e: Event) {
  e.stopPropagation()
  if (props.todo.isPinned) {
    await todoStore.unpinTodo(props.todo.id)
  } else {
    await todoStore.pinTodo(props.todo.id)
  }
}

// 点击待办
function handleClick() {
  emit('click')
}
</script>

<template>
  <div 
    class="todo-item" 
    :class="{ completed: isCompleted }"
    @click="handleClick"
  >
    <!-- 拖拽手柄 + 颜色圆点 -->
    <div class="drag-handle color-dot" :style="colorStyle"></div>

    <!-- 内容区域 -->
    <div class="todo-content">
      <div class="todo-title">{{ todo.title }}</div>
      
      <div v-if="subtaskStats.total > 0 || formattedNotifyTime" class="todo-meta">
        <!-- 子任务统计 -->
        <span v-if="subtaskStats.total > 0" class="subtask-count">
          <el-icon :size="12"><Finished /></el-icon>
          {{ subtaskStats.completed }}/{{ subtaskStats.total }}
        </span>

        <!-- 通知时间 -->
        <span v-if="formattedNotifyTime" class="notify-time">
          <el-icon :size="12"><Bell /></el-icon>
          {{ formattedNotifyTime }}
        </span>
      </div>
    </div>

    <!-- 置顶图标（在标题旁显示） -->
    <el-icon
      v-if="todo.isPinned && !isCompleted"
      class="pin-indicator"
      :size="14"
      title="已置顶"
    >
      <svg viewBox="0 0 24 24" fill="currentColor" width="14" height="14">
        <path d="M16 12V4H17V2H7V4H8V12L6 14V16H11.2V22H12.8V16H18V14L16 12Z"/>
      </svg>
    </el-icon>

    <!-- 操作按钮 -->
    <div class="todo-actions">
      <button
        class="action-btn complete-btn"
        :title="isCompleted ? '取消完成' : '完成'"
        @click="toggleComplete"
      >
        <el-icon :size="16">
          <Select v-if="!isCompleted" />
          <RefreshLeft v-else />
        </el-icon>
      </button>

      <button
        v-if="!isCompleted"
        class="action-btn pin-btn"
        :class="{ pinned: todo.isPinned }"
        :title="todo.isPinned ? '取消置顶' : '置顶'"
        @click="togglePin"
      >
        <el-icon :size="16">
          <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
            <path d="M16 12V4H17V2H7V4H8V12L6 14V16H11.2V22H12.8V16H18V14L16 12Z"/>
          </svg>
        </el-icon>
      </button>

      <button
        class="action-btn delete-btn"
        title="删除"
        @click="deleteTodo"
      >
        <el-icon :size="16"><Delete /></el-icon>
      </button>
    </div>
  </div>
</template>

<style scoped>
/* 使用 main.scss 中的样式 */
</style>
