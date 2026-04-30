import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
import dayjs from 'dayjs'
import type { Todo, CreateTodoRequest, UpdateTodoRequest, SubTask, CreateSubTaskRequest, UpdateSubTaskRequest, ViewMode, QuadrantType, ViewRange } from '@/types'
import { QUADRANTS, DEFAULT_VIEW_RANGE } from '@/types'

const DEFAULT_VIEW_MODE: ViewMode = 'quadrant'
const VIEW_RANGE_STORAGE_KEY = 'mini-todo-view-range'

function getOccurrenceScore(todo: Todo) {
  if (!todo.startTime) return Number.POSITIVE_INFINITY
  const parsed = dayjs(todo.startTime)
  return parsed.isValid() ? parsed.valueOf() : Number.POSITIVE_INFINITY
}

function comparePendingTodos(a: Todo, b: Todo) {
  if (a.isPinned !== b.isPinned) {
    return a.isPinned ? -1 : 1
  }

  if (a.isPinned && b.isPinned) {
    return a.sortOrder - b.sortOrder || a.id - b.id
  }

  const aTime = getOccurrenceScore(a)
  const bTime = getOccurrenceScore(b)
  if (aTime !== bTime) {
    return aTime - bTime
  }

  return a.sortOrder - b.sortOrder || a.id - b.id
}

function loadViewRangeFromStorage(): ViewRange {
  try {
    const stored = localStorage.getItem(VIEW_RANGE_STORAGE_KEY)
    if (stored === '7D' || stored === '3D' || stored === '1D' || stored === 'ALL') {
      return stored
    }
  } catch {
    // localStorage 不可用
  }
  return DEFAULT_VIEW_RANGE
}

function saveViewRangeToStorage(range: ViewRange) {
  try {
    localStorage.setItem(VIEW_RANGE_STORAGE_KEY, range)
  } catch {
    // localStorage 不可用
  }
}

function isTodoInViewRange(todo: Todo, viewRange: ViewRange): boolean {
  // ALL: 显示所有事项
  if (viewRange === 'ALL') return true

  // 无开始时间的事项始终显示（不隐藏）
  if (!todo.startTime) return true

  const startDate = dayjs(todo.startTime)
  if (!startDate.isValid()) return true

  const today = dayjs().startOf('day')
  const todoDate = startDate.startOf('day')

  // 计算事项开始日期与今天的差值（天数）
  const diffDays = todoDate.diff(today, 'day')

  switch (viewRange) {
    case '1D':
      // 1D: 只显示今天及之前的（未完成的过去事项也要显示）
      return diffDays <= 0
    case '3D':
      // 3D: 显示今天起未来3天内（今天、明天、后天）以及所有过去的事项
      return diffDays <= 2
    case '7D':
      // 7D: 显示今天起未来7天内以及所有过去的事项
      return diffDays <= 6
    default:
      return true
  }
}

export const useTodoStore = defineStore('todo', () => {
  // 状态
  const todos = ref<Todo[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const viewMode = ref<ViewMode>(DEFAULT_VIEW_MODE)
  const viewRange = ref<ViewRange>(loadViewRangeFromStorage())

  function notifyLocalChanged() {
    emit('todo-local-changed').catch(() => undefined)
  }

  // 经过时间范围过滤的待办
  const filteredTodos = computed(() => {
    return todos.value.filter(t => isTodoInViewRange(t, viewRange.value))
  })

  // 计算属性
  const pendingTodos = computed(() =>
    filteredTodos.value
      .filter(t => !t.completed)
      .sort(comparePendingTodos)
  )

  const completedTodos = computed(() =>
    filteredTodos.value
      .filter(t => t.completed)
      .sort((a, b) => b.sortOrder - a.sortOrder)
  )

  const todoCount = computed(() => ({
    total: todos.value.length,
    pending: pendingTodos.value.length,
    completed: completedTodos.value.length
  }))

  // 按象限分组的待办（仅未完成，已过滤）
  const todosByQuadrant = computed(() => {
    const result: Record<QuadrantType, Todo[]> = {
      [QUADRANTS.IMPORTANT_URGENT]: [],
      [QUADRANTS.IMPORTANT_NOT_URGENT]: [],
      [QUADRANTS.URGENT_NOT_IMPORTANT]: [],
      [QUADRANTS.NOT_URGENT_NOT_IMPORTANT]: [],
    }

    pendingTodos.value.forEach(todo => {
      const quadrant = todo.quadrant as QuadrantType
      if (result[quadrant]) {
        result[quadrant].push(todo)
      } else {
        // 默认放入第一象限
        result[QUADRANTS.IMPORTANT_URGENT].push(todo)
      }
    })

    return result
  })

  // 操作方法
  async function fetchTodos() {
    loading.value = true
    error.value = null
    try {
      todos.value = await invoke<Todo[]>('get_todos')
    } catch (e) {
      error.value = String(e)
      console.error('Failed to fetch todos:', e)
    } finally {
      loading.value = false
    }
  }

  async function addTodo(data: CreateTodoRequest): Promise<Todo | null> {
    try {
      const newTodo = await invoke<Todo>('create_todo', { data })
      todos.value.push(newTodo)
      notifyLocalChanged()
      return newTodo
    } catch (e) {
      error.value = String(e)
      console.error('Failed to add todo:', e)
      return null
    }
  }

  async function updateTodo(id: number, data: UpdateTodoRequest): Promise<boolean> {
    try {
      const updatedTodo = await invoke<Todo>('update_todo', { id, data })
      const index = todos.value.findIndex(t => t.id === id)
      if (index !== -1) {
        todos.value[index] = updatedTodo
      }
      notifyLocalChanged()
      return true
    } catch (e) {
      error.value = String(e)
      console.error('Failed to update todo:', e)
      return false
    }
  }

  async function deleteTodo(id: number): Promise<boolean> {
    try {
      await invoke('delete_todo', { id })
      todos.value = todos.value.filter(t => t.id !== id)
      notifyLocalChanged()
      return true
    } catch (e) {
      error.value = String(e)
      console.error('Failed to delete todo:', e)
      return false
    }
  }

  async function toggleComplete(id: number): Promise<boolean> {
    const todo = todos.value.find(t => t.id === id)
    if (!todo) return false
    return updateTodo(id, { completed: !todo.completed })
  }

  async function reorderTodos(orderedIds: number[]): Promise<boolean> {
    try {
      await invoke('reorder_todos', { ids: orderedIds })
      // 更新本地排序
      orderedIds.forEach((id, index) => {
        const todo = todos.value.find(t => t.id === id)
        if (todo) {
          todo.sortOrder = index
        }
      })
      notifyLocalChanged()
      return true
    } catch (e) {
      error.value = String(e)
      console.error('Failed to reorder todos:', e)
      return false
    }
  }

  async function pinTodo(id: number): Promise<boolean> {
    const todo = todos.value.find(t => t.id === id)
    if (!todo) return false

    const pinnedOrders = todos.value
      .filter(t => !t.completed && t.isPinned && t.id !== id)
      .map(t => t.sortOrder)
    const nextSortOrder = Math.min(0, ...pinnedOrders) - 1
    return updateTodo(id, { isPinned: true, sortOrder: nextSortOrder })
  }

  async function unpinTodo(id: number): Promise<boolean> {
    const todo = todos.value.find(t => t.id === id)
    if (!todo) return false
    return updateTodo(id, { isPinned: false })
  }

  // 更新待办的象限
  async function updateTodoQuadrant(id: number, quadrant: QuadrantType): Promise<boolean> {
    return updateTodo(id, { quadrant })
  }

  // 设置视图模式
  function setViewMode(mode: ViewMode) {
    viewMode.value = mode
  }

  // 视图模式仅在当前进程内保留；每次应用启动默认四象限。
  async function loadViewMode(useStartupDefault = false) {
    if (useStartupDefault) {
      viewMode.value = DEFAULT_VIEW_MODE
    }
  }

  // 保留方法给标题栏调用，但不持久化到本机数据库或 WebDAV。
  async function saveViewMode() {
    return
  }

  // 设置时间范围视图
  function setViewRange(range: ViewRange) {
    viewRange.value = range
    saveViewRangeToStorage(range)
  }

  // 循环切换时间范围
  function cycleViewRange() {
    const ranges: ViewRange[] = ['7D', '3D', '1D', 'ALL']
    const currentIndex = ranges.indexOf(viewRange.value)
    const nextIndex = (currentIndex + 1) % ranges.length
    setViewRange(ranges[nextIndex])
  }

  // 子任务操作
  async function addSubTask(data: CreateSubTaskRequest): Promise<SubTask | null> {
    try {
      const newSubTask = await invoke<SubTask>('create_subtask', { data })
      const todo = todos.value.find(t => t.id === data.parentId)
      if (todo) {
        todo.subtasks.push(newSubTask)
      }
      notifyLocalChanged()
      return newSubTask
    } catch (e) {
      error.value = String(e)
      console.error('Failed to add subtask:', e)
      return null
    }
  }

  async function updateSubTask(id: number, data: UpdateSubTaskRequest): Promise<boolean> {
    try {
      const updatedSubTask = await invoke<SubTask>('update_subtask', { id, data })
      for (const todo of todos.value) {
        const index = todo.subtasks.findIndex(s => s.id === id)
        if (index !== -1) {
          todo.subtasks[index] = updatedSubTask
          break
        }
      }
      notifyLocalChanged()
      return true
    } catch (e) {
      error.value = String(e)
      console.error('Failed to update subtask:', e)
      return false
    }
  }

  async function deleteSubTask(id: number): Promise<boolean> {
    try {
      await invoke('delete_subtask', { id })
      for (const todo of todos.value) {
        const index = todo.subtasks.findIndex(s => s.id === id)
        if (index !== -1) {
          todo.subtasks.splice(index, 1)
          break
        }
      }
      notifyLocalChanged()
      return true
    } catch (e) {
      error.value = String(e)
      console.error('Failed to delete subtask:', e)
      return false
    }
  }

  async function toggleSubTaskComplete(id: number): Promise<boolean> {
    for (const todo of todos.value) {
      const subtask = todo.subtasks.find(s => s.id === id)
      if (subtask) {
        return updateSubTask(id, { completed: !subtask.completed })
      }
    }
    return false
  }

  return {
    // 状态
    todos,
    loading,
    error,
    viewMode,
    viewRange,
    // 计算属性
    pendingTodos,
    completedTodos,
    todoCount,
    todosByQuadrant,
    // 方法
    fetchTodos,
    addTodo,
    updateTodo,
    deleteTodo,
    toggleComplete,
    reorderTodos,
    pinTodo,
    unpinTodo,
    updateTodoQuadrant,
    setViewMode,
    loadViewMode,
    saveViewMode,
    setViewRange,
    cycleViewRange,
    addSubTask,
    updateSubTask,
    deleteSubTask,
    toggleSubTaskComplete
  }
})
