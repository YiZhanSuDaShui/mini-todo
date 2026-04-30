<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'

const trayMenuWindow = getCurrentWindow()
let canCloseOnBlur = false
let blurTimer: number | null = null

async function closeMenu() {
  try {
    await trayMenuWindow.close()
  } catch (e) {
    console.error('关闭托盘菜单失败:', e)
  }
}

function closeMenuSoon() {
  if (!canCloseOnBlur) return

  if (blurTimer) {
    window.clearTimeout(blurTimer)
  }

  blurTimer = window.setTimeout(() => {
    closeMenu()
  }, 80)
}

async function showMainWindow() {
  try {
    await invoke('show_main_window')
  } finally {
    await closeMenu()
  }
}

async function exitApp() {
  await invoke('exit_app')
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    closeMenu()
  }
}

onMounted(() => {
  if (trayMenuWindow.label !== 'tray-menu') {
    console.error(`托盘菜单页面被加载到了错误窗口中: ${trayMenuWindow.label}`)
    closeMenu()
    return
  }

  window.addEventListener('keydown', handleKeydown)
  window.addEventListener('blur', closeMenuSoon)

  window.setTimeout(() => {
    canCloseOnBlur = true
  }, 200)
})

onUnmounted(() => {
  if (blurTimer) {
    window.clearTimeout(blurTimer)
  }
  window.removeEventListener('keydown', handleKeydown)
  window.removeEventListener('blur', closeMenuSoon)
})
</script>

<template>
  <div class="tray-menu-shell">
    <button class="tray-menu-item" type="button" @click="showMainWindow">
      <span class="item-dot"></span>
      <span>展开界面</span>
    </button>
    <button class="tray-menu-item danger" type="button" @click="exitApp">
      <span class="item-dot"></span>
      <span>退出</span>
    </button>
  </div>
</template>

<style scoped>
.tray-menu-shell {
  width: 100vw;
  height: 100vh;
  box-sizing: border-box;
  padding: 6px;
  background: rgba(255, 255, 255, 0.96);
  border: 1px solid rgba(15, 23, 42, 0.1);
  border-radius: 10px;
  box-shadow:
    0 16px 36px rgba(15, 23, 42, 0.16),
    0 4px 12px rgba(15, 23, 42, 0.1);
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow: hidden;
}

.tray-menu-item {
  width: 100%;
  height: 39px;
  border: 0;
  border-radius: 7px;
  background: transparent;
  color: #1e293b;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 12px;
  font-size: 13px;
  font-weight: 500;
  text-align: left;
  cursor: pointer;
  transition: background 0.14s ease, color 0.14s ease;
}

.tray-menu-item:hover,
.tray-menu-item:focus-visible {
  outline: none;
  background: #f1f5f9;
}

.tray-menu-item.danger {
  color: #b91c1c;
}

.tray-menu-item.danger:hover,
.tray-menu-item.danger:focus-visible {
  background: #fef2f2;
}

.item-dot {
  width: 6px;
  height: 6px;
  border-radius: 999px;
  background: currentColor;
  opacity: 0.45;
  flex: 0 0 auto;
}
</style>
