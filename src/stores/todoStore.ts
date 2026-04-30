import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
import dayjs from 'dayjs'
import type { Todo, CreateTodoRequest, UpdateTodoRequest, SubTask, CreateSubTaskRequest, UpdateSubTaskRequest, ViewMode, QuadrantType } from '@/types'
import { QUADRANTS } from '@/types'

const DEFAULT_VIEW_MODE: ViewMode = 'quadrant'

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

export const useTodoStore = defineStore('todo', () => {
  // 状态
  const todos = ref<Todo[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const viewMode = ref<ViewMode>(DEFAULT_VIEW_MODE)

  function notifyLocalChanged() {
    emit('todo-local-changed').catch(() => undefined)
  }

  // 计算属性
  const pendingTodos = computed(() =>
    todos.value
      .filter(t => !t.completed)
      .sort(comparePendingTodos)
  )

  const completedTodos = computed(() => 
    todos.value
      .filter(t => t.completed)
      .sort((a, b) => b.sortOrder - a.sortOrder)
  )

  const todoCount = computed(() => ({
    total: todos.value.length,
    pending: pendingTodos.value.length,
    completed: completedTodos.value.length
  }))

  // 按象限分组的待办（仅未完成）
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
    updateTodoQuadrant,
    setViewMode,
    loadViewMode,
    saveViewMode,
    addSubTask,
    updateSubTask,
    deleteSubTask,
    toggleSubTaskComplete
  }
})
