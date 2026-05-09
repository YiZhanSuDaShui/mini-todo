<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { availableMonitors, cursorPosition, getCurrentWindow, PhysicalPosition } from '@tauri-apps/api/window'
import miniTodoLogo from '@/assets/minitodo-logo.png'

const bubbleWindow = getCurrentWindow()

const EDGE_GAP = 30
const BALL_SIZE = 48
const DRAG_THRESHOLD = 5
const SNAP_DELAY_MS = 40
const SNAP_ANIMATION_MS = 320
const CLICK_SUPPRESS_AFTER_DRAG_MS = 450

let unlistenMoved: (() => void) | null = null
let snapTimer: number | null = null
let snapAnimationFrame: number | null = null
let windowMoveInFlight = false
let pendingWindowPosition: { x: number; y: number } | null = null
let lastPositionErrorAt = 0
let snapping = false
let isMouseDown = false
let isDragging = false
let isPreparingDrag = false
let dragSessionId = 0
let activePointerId: number | null = null
let pointerCaptureElement: HTMLElement | null = null
let dragStartScreenX = 0
let dragStartScreenY = 0
let latestDragScreenX = 0
let latestDragScreenY = 0
let dragScaleFactor = 1
let dragMoveFrame: number | null = null
let latestDragCursorX = 0
let latestDragCursorY = 0
let mainWindowShownByBubble = false
let lastToggleAt = 0
let togglingMainWindow = false
let movedBeyondClickThreshold = false
let suppressClickUntil = 0
let dragStart: {
  offsetX: number
  offsetY: number
} | null = null

const pressing = ref(false)
const dragging = ref(false)
const snappingActive = ref(false)

function clamp(value: number, min: number, max: number) {
  if (max < min) return min
  return Math.min(Math.max(value, min), max)
}

function toPhysicalCursor(event: MouseEvent) {
  return {
    x: Math.round(event.screenX * dragScaleFactor),
    y: Math.round(event.screenY * dragScaleFactor)
  }
}

function clearSnapTimer() {
  if (snapTimer) {
    window.clearTimeout(snapTimer)
    snapTimer = null
  }
}

function cancelSnapAnimation() {
  if (snapAnimationFrame) {
    window.cancelAnimationFrame(snapAnimationFrame)
    snapAnimationFrame = null
  }
  pendingWindowPosition = null
  snapping = false
  snappingActive.value = false
}

function cancelDragMoveFrame() {
  if (dragMoveFrame) {
    window.cancelAnimationFrame(dragMoveFrame)
    dragMoveFrame = null
  }
}

function releasePointerCapture() {
  if (activePointerId === null || !pointerCaptureElement) {
    activePointerId = null
    pointerCaptureElement = null
    return
  }

  try {
    if (pointerCaptureElement.hasPointerCapture(activePointerId)) {
      pointerCaptureElement.releasePointerCapture(activePointerId)
    }
  } catch {
    // 指针捕获可能已经被系统释放，忽略即可。
  }

  activePointerId = null
  pointerCaptureElement = null
}

function reportPositionError(message: string, error: unknown) {
  const now = performance.now()
  if (now - lastPositionErrorAt < 1000) return
  lastPositionErrorAt = now
  console.error(message, error)
}

async function reinforceTopmost() {
  await bubbleWindow.setAlwaysOnTop(true).catch(() => undefined)
  await bubbleWindow.setSkipTaskbar(true).catch(() => undefined)
  await invoke('reinforce_floating_bubble_topmost').catch((e) => {
    console.warn('强化悬浮球置顶失败:', e)
  })
}

function flushWindowPosition() {
  if (windowMoveInFlight || !pendingWindowPosition) return

  const next = pendingWindowPosition
  pendingWindowPosition = null
  windowMoveInFlight = true

  void bubbleWindow.setPosition(new PhysicalPosition(next.x, next.y))
    .catch((e) => {
      reportPositionError('移动悬浮球失败:', e)
    })
    .finally(() => {
      windowMoveInFlight = false
      if (pendingWindowPosition) {
        window.requestAnimationFrame(flushWindowPosition)
      }
    })
}

function requestWindowPosition(x: number, y: number) {
  pendingWindowPosition = {
    x: Math.round(x),
    y: Math.round(y)
  }
  flushWindowPosition()
}

function resetDragState() {
  dragSessionId += 1
  isMouseDown = false
  isDragging = false
  isPreparingDrag = false
  movedBeyondClickThreshold = false
  dragStart = null
  releasePointerCapture()
  cancelDragMoveFrame()
  pressing.value = false
  dragging.value = false
}

function springEase(t: number) {
  if (t >= 1) return 1
  return 1 - Math.exp(-7 * t) * Math.cos(t * Math.PI * 4.5)
}

function animateWindowPosition(
  fromX: number,
  fromY: number,
  toX: number,
  toY: number
) {
  cancelSnapAnimation()

  const startedAt = performance.now()
  snapping = true
  snappingActive.value = true

  const tick = (now: number) => {
    const progress = clamp((now - startedAt) / SNAP_ANIMATION_MS, 0, 1)
    const eased = springEase(progress)
    const x = Math.round(fromX + (toX - fromX) * eased)
    const y = Math.round(fromY + (toY - fromY) * eased)

    requestWindowPosition(x, y)

    if (progress < 1) {
      snapAnimationFrame = window.requestAnimationFrame(tick)
      return
    }

    snapAnimationFrame = null
    requestWindowPosition(toX, toY)
    window.setTimeout(() => {
      snapping = false
      snappingActive.value = false
      void reinforceTopmost()
    }, 80)
  }

  snapAnimationFrame = window.requestAnimationFrame(tick)
}

async function getMonitorForBubble() {
  const bubblePosition = await bubbleWindow.outerPosition()
  const bubbleSize = await bubbleWindow.outerSize()
  const monitors = await availableMonitors()
  const bubbleCenterX = bubblePosition.x + bubbleSize.width / 2
  const bubbleCenterY = bubblePosition.y + bubbleSize.height / 2

  const monitor = monitors.find((item) => {
    const left = item.position.x
    const right = item.position.x + item.size.width
    const top = item.position.y
    const bottom = item.position.y + item.size.height
    return bubbleCenterX >= left && bubbleCenterX <= right && bubbleCenterY >= top && bubbleCenterY <= bottom
  }) || monitors[0]

  return { bubblePosition, bubbleSize, monitor }
}

async function snapToEdge(preferredSide?: 'left' | 'right') {
  if (snapping) return

  try {
    const { bubblePosition, bubbleSize, monitor } = await getMonitorForBubble()
    if (!monitor) return

    const scale = monitor.scaleFactor || 1
    const gap = Math.round(EDGE_GAP * scale)
    const middleX = monitor.position.x + monitor.size.width / 2
    const bubbleCenterX = bubblePosition.x + bubbleSize.width / 2
    const side = preferredSide || (bubbleCenterX < middleX ? 'left' : 'right')
    const x = side === 'left'
      ? monitor.position.x + gap
      : monitor.position.x + monitor.size.width - bubbleSize.width - gap
    const y = clamp(
      bubblePosition.y,
      monitor.position.y + gap,
      monitor.position.y + monitor.size.height - bubbleSize.height - gap
    )

    animateWindowPosition(
      bubblePosition.x,
      bubblePosition.y,
      Math.round(x),
      Math.round(y)
    )
  } catch (e) {
    console.error('吸附悬浮球失败:', e)
  }
}

function scheduleSnap() {
  clearSnapTimer()
  snapTimer = window.setTimeout(() => {
    snapTimer = null
    snapToEdge()
  }, SNAP_DELAY_MS)
}

async function toggleMainWindow() {
  const now = performance.now()
  if (togglingMainWindow) return
  if (now - lastToggleAt < 350) return
  lastToggleAt = now
  togglingMainWindow = true

  try {
    try {
      mainWindowShownByBubble = await invoke<boolean>('toggle_main_window')
      void reinforceTopmost()
      return
    } catch (e) {
      console.warn('后端切换主窗口命令不可用，使用前端兜底:', e)
    }

    if (mainWindowShownByBubble) {
      await invoke('hide_main_window')
      mainWindowShownByBubble = false
      return
    }

    await invoke('show_main_window')
    mainWindowShownByBubble = true
    void reinforceTopmost()
  } finally {
    togglingMainWindow = false
  }
}

function handlePointerDown(event: PointerEvent) {
  if (event.button !== 0) return

  event.preventDefault()
  clearSnapTimer()
  cancelSnapAnimation()

  const sessionId = dragSessionId + 1
  dragSessionId = sessionId
  isMouseDown = true
  isDragging = false
  isPreparingDrag = false
  movedBeyondClickThreshold = false
  dragStart = null
  dragStartScreenX = event.screenX
  dragStartScreenY = event.screenY
  latestDragScreenX = event.screenX
  latestDragScreenY = event.screenY
  pressing.value = true
  dragging.value = false

  activePointerId = event.pointerId
  pointerCaptureElement = event.currentTarget instanceof HTMLElement ? event.currentTarget : null
  try {
    pointerCaptureElement?.setPointerCapture(event.pointerId)
  } catch {
    // 某些 WebView 场景可能不支持捕获，后续仍有 document 级事件兜底。
  }
}

function scheduleDragMove(cursorX: number, cursorY: number) {
  latestDragCursorX = cursorX
  latestDragCursorY = cursorY

  if (dragMoveFrame) return

  dragMoveFrame = window.requestAnimationFrame(() => {
    dragMoveFrame = null
    if (!isMouseDown || !isDragging || !dragStart) return

    requestWindowPosition(
      latestDragCursorX - dragStart.offsetX,
      latestDragCursorY - dragStart.offsetY
    )
  })
}

function prepareDrag(sessionId: number) {
  if (isPreparingDrag) return
  isPreparingDrag = true

  Promise.all([
    cursorPosition(),
    bubbleWindow.outerPosition(),
    bubbleWindow.scaleFactor().catch(() => 1)
  ]).then(([cursor, position, scale]) => {
    if (!isMouseDown || sessionId !== dragSessionId) return

    dragScaleFactor = scale || 1
    dragStart = {
      offsetX: cursor.x - position.x,
      offsetY: cursor.y - position.y
    }
    isDragging = true
    isPreparingDrag = false
    pressing.value = false
    dragging.value = true

    scheduleDragMove(
      Math.round(latestDragScreenX * dragScaleFactor),
      Math.round(latestDragScreenY * dragScaleFactor)
    )
  }).catch((e) => {
    if (sessionId !== dragSessionId) return
    console.error('准备拖拽悬浮球失败:', e)
    resetDragState()
  })
}

function handlePointerMove(event: PointerEvent) {
  if (!isMouseDown) return
  if (activePointerId !== null && event.pointerId !== activePointerId) return

  latestDragScreenX = event.screenX
  latestDragScreenY = event.screenY

  if ((event.buttons & 1) !== 1) {
    resetDragState()
    return
  }

  const deltaX = event.screenX - dragStartScreenX
  const deltaY = event.screenY - dragStartScreenY

  if (!isDragging && Math.hypot(deltaX, deltaY) < DRAG_THRESHOLD) return
  movedBeyondClickThreshold = true

  const sessionId = dragSessionId
  if (!dragStart) {
    prepareDrag(sessionId)
    return
  }

  if (!isMouseDown || !isDragging || sessionId !== dragSessionId) return
  const cursor = toPhysicalCursor(event)
  scheduleDragMove(cursor.x, cursor.y)
}

function handlePointerCancel(event: PointerEvent) {
  if (activePointerId === null || event.pointerId === activePointerId) {
    resetDragState()
  }
}

async function handlePointerUp(event: PointerEvent) {
  if (!isMouseDown) return
  if (activePointerId !== null && event.pointerId !== activePointerId) return

  const shouldTreatAsDrag = isDragging || isPreparingDrag || movedBeyondClickThreshold
  resetDragState()

  if (shouldTreatAsDrag) {
    suppressClickUntil = performance.now() + CLICK_SUPPRESS_AFTER_DRAG_MS
    scheduleSnap()
    void reinforceTopmost()
    return
  }

  await toggleMainWindow()
}

async function handleBubbleClick(event: MouseEvent) {
  event.preventDefault()
  event.stopPropagation()

  if (performance.now() < suppressClickUntil) return
  if (isDragging || dragging.value) return
  await toggleMainWindow()
}

function handleContextMenu(event: MouseEvent) {
  event.preventDefault()
}

onMounted(async () => {
  if (bubbleWindow.label !== 'fixed-bubble') {
    console.error(`悬浮球页面被加载到了错误窗口中: ${bubbleWindow.label}`)
    return
  }

  document.addEventListener('pointermove', handlePointerMove)
  document.addEventListener('pointerup', handlePointerUp)
  document.addEventListener('pointercancel', handlePointerCancel)
  document.addEventListener('contextmenu', handleContextMenu)
  window.addEventListener('blur', resetDragState)

  unlistenMoved = await bubbleWindow.onMoved(() => {
    if (snapping || isMouseDown || isDragging) return
    scheduleSnap()
  })

  await reinforceTopmost()
  const scale = await bubbleWindow.scaleFactor().catch(() => 1)
  const physicalSize = Math.round(BALL_SIZE * scale)
  await invoke('set_window_exact_size_by_label', {
    label: 'fixed-bubble',
    width: physicalSize,
    height: physicalSize
  }).catch((e) => {
    console.error('按标签校正悬浮球窗口尺寸失败:', e)
  })
  await reinforceTopmost()
})

onUnmounted(() => {
  document.removeEventListener('pointermove', handlePointerMove)
  document.removeEventListener('pointerup', handlePointerUp)
  document.removeEventListener('pointercancel', handlePointerCancel)
  document.removeEventListener('contextmenu', handleContextMenu)
  window.removeEventListener('blur', resetDragState)
  if (unlistenMoved) unlistenMoved()
  clearSnapTimer()
  cancelSnapAnimation()
  cancelDragMoveFrame()
  pendingWindowPosition = null
})
</script>

<template>
  <div
    class="floating-ball"
    :class="{ pressing, dragging, snapping: snappingActive }"
    role="button"
    title="Mini Todo"
    @pointerdown="handlePointerDown"
    @click="handleBubbleClick"
  >
    <div class="floating-ball-inner">
      <img class="floating-ball-logo" :src="miniTodoLogo" alt="Mini Todo" draggable="false">
    </div>
  </div>
</template>

<style scoped>
:global(html),
:global(body),
:global(#app) {
  width: 100%;
  height: 100%;
  margin: 0;
  padding: 0;
  overflow: hidden;
  background: transparent;
  user-select: none;
  -webkit-user-select: none;
}

.floating-ball {
  --logo-mask-size: 48px;
  --logo-center-x-adjust: -0.1px;
  --logo-center-y-adjust: 0.1px;

  width: 48px;
  height: 48px;
  border-radius: 50%;
  position: relative;
  cursor: pointer;
  transition: transform 0.15s ease;
  will-change: transform, opacity;
  contain: layout paint size style;
  transform: translateZ(0);
  touch-action: none;
  -webkit-app-region: no-drag;
  isolation: isolate;
}

.floating-ball::after {
  content: '';
  position: absolute;
  left: 50%;
  top: 50%;
  width: var(--logo-mask-size);
  height: var(--logo-mask-size);
  border-radius: 50%;
  background: rgba(15, 23, 42, 0.22);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.16s ease;
  transform: translate(-50%, -50%)
    translate(var(--logo-center-x-adjust), var(--logo-center-y-adjust));
  z-index: 2;
}

.floating-ball-inner {
  position: absolute;
  inset: 0;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  transform: translateZ(0);
  will-change: transform;
  z-index: 1;
}

.floating-ball-logo {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
  object-position: center;
  user-select: none;
  -webkit-user-drag: none;
  pointer-events: none;
  backface-visibility: hidden;
  transform: translateZ(0);
}

.floating-ball:hover::after {
  opacity: 1;
}

.floating-ball.pressing {
  transform: scale(0.92);
}

.floating-ball.dragging {
  transform: scale(1);
  opacity: 0.8;
  transition: none;
}

.floating-ball.snapping .floating-ball-inner {
  animation: snap-pop 360ms cubic-bezier(0.18, 1.35, 0.32, 1);
}

@keyframes snap-pop {
  0% {
    transform: scale(0.96);
  }

  55% {
    transform: scale(1.08);
  }

  100% {
    transform: scale(1);
  }
}
</style>
