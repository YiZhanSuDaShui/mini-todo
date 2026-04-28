import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { invoke } from '@tauri-apps/api/core'
import { emit as emitTauriEvent, listen } from '@tauri-apps/api/event'
import { getCurrentWindow, PhysicalPosition, PhysicalSize, availableMonitors, primaryMonitor } from '@tauri-apps/api/window'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import packageJson from '../../package.json'
import type { WindowPosition, WindowSize, WindowMode, ScreenConfig, SaveScreenConfigRequest, MonitorInfo } from '@/types'

// 当前应用版本（从系统读取）
const APP_VERSION_FALLBACK = packageJson.version || '1.6.3'
export const APP_VERSION = ref<string>(APP_VERSION_FALLBACK)
// GitHub 仓库信息：用于检查更新和打开 Release 页面
const GITHUB_OWNER = 'YiZhanSuDaShui'
const GITHUB_REPO = 'mini-todo'
const GITHUB_RELEASES_URL = `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}/releases`
const FLOATING_BUBBLE_LABEL = 'fixed-bubble'
const FLOATING_BUBBLE_SIZE = 48
const FLOATING_BUBBLE_MARGIN = 30
const FLOATING_BUBBLE_STATE_EVENT = 'floating-bubble-state-changed'

interface WindowPersistState {
  position: WindowPosition
  size: WindowSize
}

interface AppSettingsSnapshot {
  isFixed: boolean
  windowPosition: WindowPosition | null
  windowSize: WindowSize | null
  autoHideEnabled: boolean
  textTheme: string
  showCalendar: boolean
  viewMode: string
  notificationType: string
}

interface FloatingBubbleStatePayload {
  enabled: boolean
  source: string
}

interface UpdateAsset {
  name: string
  downloadUrl: string
}

interface LatestReleaseInfo {
  tagName: string
  releaseUrl: string
  installerAsset: UpdateAsset | null
}

interface UpdateDownloadResult {
  filePath: string
  fileName: string
  bytes: number
}

export const useAppStore = defineStore('app', () => {
  function applyPlatformClass() {
    const ua = navigator.userAgent.toLowerCase()
    const isMacOS = ua.includes('macintosh') || ua.includes('mac os x')
    document.body.classList.toggle('platform-macos', isMacOS)
  }

  async function loadAppVersion() {
    try {
      const version = await getVersion()
      APP_VERSION.value = version || APP_VERSION_FALLBACK
    } catch (e) {
      console.error('Failed to load app version:', e)
      APP_VERSION.value = APP_VERSION_FALLBACK
    }
  }

  // 状态
  const isFixed = ref(false)
  const isDarkTheme = ref(false)
  const windowPosition = ref<WindowPosition | null>(null)
  const windowSize = ref<WindowSize | null>(null)
  const windowMode = ref<WindowMode>('normal')
  
  // 屏幕配置相关状态
  const currentScreenConfigId = ref<string>('')
  const screenConfigs = ref<ScreenConfig[]>([])
  
  // 日历显示状态
  const showCalendar = ref(false)
  // 是否启用贴边自动隐藏
  const autoHideEnabled = ref(true)
  
  // 版本更新相关状态
  const hasUpdate = ref(false)
  const latestVersion = ref<string | null>(null)
  const releaseUrl = ref<string | null>(null)
  const updateInstallerAsset = ref<UpdateAsset | null>(null)
  const updateCheckError = ref<string | null>(null)

  // 获取当前窗口
  const appWindow = getCurrentWindow()
  let closingFloatingBubbleInternally = false
  let floatingBubbleDestroyBound = false
  let unlistenFloatingBubbleState: (() => void) | null = null

  function setFloatingBubbleStateLocal(enabled: boolean) {
    isFixed.value = enabled
    windowMode.value = enabled ? 'fixed' : 'normal'
  }

  async function broadcastFloatingBubbleState(enabled: boolean) {
    await emitTauriEvent(FLOATING_BUBBLE_STATE_EVENT, {
      enabled,
      source: appWindow.label
    } satisfies FloatingBubbleStatePayload).catch((e) => {
      console.warn('Failed to broadcast floating bubble state:', e)
    })
  }

  async function listenFloatingBubbleStateChanges() {
    if (unlistenFloatingBubbleState) return

    unlistenFloatingBubbleState = await listen<FloatingBubbleStatePayload>(
      FLOATING_BUBBLE_STATE_EVENT,
      ({ payload }) => {
        if (!payload || payload.source === appWindow.label) return

        setFloatingBubbleStateLocal(payload.enabled)
        void syncFloatingBubbleChecked(payload.enabled)

        if (appWindow.label === 'main') {
          void saveWindowState()
        }
      }
    )
  }

  async function getPersistedFloatingBubbleEnabled(fallback: boolean) {
    try {
      const settings = await invoke<AppSettingsSnapshot>('get_settings')
      return settings.isFixed
    } catch (e) {
      console.warn('Failed to load persisted floating bubble state:', e)
      return fallback
    }
  }

  async function loadFloatingBubbleEnabled() {
    const enabled = await getPersistedFloatingBubbleEnabled(false)
    setFloatingBubbleStateLocal(enabled)
    await syncFloatingBubbleChecked(enabled)
  }

  async function saveFloatingBubbleSetting(enabled: boolean) {
    const settings = await invoke<AppSettingsSnapshot>('get_settings')
    await invoke('save_settings', {
      settings: {
        ...settings,
        isFixed: enabled
      }
    })
  }

  async function getFloatingBubblePosition() {
    const monitor = await primaryMonitor()
    if (!monitor) {
      return { x: 1200, y: 240 }
    }

    const scale = monitor.scaleFactor || 1
    const monitorX = monitor.position.x / scale
    const monitorY = monitor.position.y / scale
    const monitorWidth = monitor.size.width / scale
    const monitorHeight = monitor.size.height / scale

    return {
      x: Math.round(monitorX + monitorWidth - FLOATING_BUBBLE_SIZE - FLOATING_BUBBLE_MARGIN),
      y: Math.round(monitorY + (monitorHeight - FLOATING_BUBBLE_SIZE) / 2)
    }
  }

  async function showMainWindow() {
    try {
      await invoke('show_main_window')
    } catch (e) {
      console.error('Failed to show main window:', e)
    }
  }

  async function syncFloatingBubbleChecked(enabled: boolean) {
    await invoke('set_window_fixed_mode', { fixed: enabled }).catch((e) => {
      console.warn('Failed to sync floating bubble checked state:', e)
    })
  }

  async function reinforceFloatingBubbleTopmost() {
    await invoke('reinforce_floating_bubble_topmost').catch((e) => {
      console.warn('Failed to reinforce floating bubble topmost state:', e)
    })
  }

  function bindFloatingBubbleDestroyed(bubble: WebviewWindow) {
    if (floatingBubbleDestroyBound) return
    floatingBubbleDestroyBound = true

    bubble.once('tauri://destroyed', () => {
      floatingBubbleDestroyBound = false
      if (closingFloatingBubbleInternally) {
        closingFloatingBubbleInternally = false
        return
      }

      if (isFixed.value) {
        setFloatingBubbleStateLocal(false)
        void syncFloatingBubbleChecked(false)
        void saveWindowState()
        void broadcastFloatingBubbleState(false)
      }
    })
  }

  async function waitForFloatingBubbleVisible(bubble: WebviewWindow, timeoutMs = 1200) {
    const startedAt = Date.now()
    await bubble.setAlwaysOnTop(true).catch(() => undefined)
    await bubble.setSkipTaskbar(true).catch(() => undefined)
    await bubble.show().catch(() => undefined)
    await reinforceFloatingBubbleTopmost()

    while (Date.now() - startedAt < timeoutMs) {
      try {
        const current = await WebviewWindow.getByLabel(FLOATING_BUBBLE_LABEL)
        const target = current || bubble

        if (await target.isVisible().catch(() => false)) {
          await reinforceFloatingBubbleTopmost()
          return true
        }
      } catch (e) {
        console.warn('Waiting for floating bubble failed:', e)
      }

      await new Promise<void>(resolve => window.setTimeout(resolve, 50))
    }

    return false
  }

  async function correctFloatingBubbleSize(bubble?: WebviewWindow) {
    const target = bubble || await WebviewWindow.getByLabel(FLOATING_BUBBLE_LABEL)
    const scale = await target?.scaleFactor().catch(() => undefined) || 1
    const physicalSize = Math.round(FLOATING_BUBBLE_SIZE * scale)

    await invoke('set_window_exact_size_by_label', {
      label: FLOATING_BUBBLE_LABEL,
      width: physicalSize,
      height: physicalSize
    }).catch((e) => {
      console.warn('Failed to correct floating bubble size:', e)
    })
  }

  async function showFloatingBubble(): Promise<boolean> {
    try {
      const existing = await WebviewWindow.getByLabel(FLOATING_BUBBLE_LABEL)

      if (existing) {
        bindFloatingBubbleDestroyed(existing)
        await existing.setAlwaysOnTop(true).catch(() => undefined)
        await existing.setSkipTaskbar(true).catch(() => undefined)
        await existing.show().catch(() => undefined)
        await correctFloatingBubbleSize(existing)
        await reinforceFloatingBubbleTopmost()
        return existing.isVisible().catch(() => true)
      }

      const position = await getFloatingBubblePosition()
      const bubble = new WebviewWindow(FLOATING_BUBBLE_LABEL, {
        url: '/#/floating-bubble',
        title: 'Mini Todo',
        width: FLOATING_BUBBLE_SIZE,
        height: FLOATING_BUBBLE_SIZE,
        minWidth: FLOATING_BUBBLE_SIZE,
        minHeight: FLOATING_BUBBLE_SIZE,
        maxWidth: FLOATING_BUBBLE_SIZE,
        maxHeight: FLOATING_BUBBLE_SIZE,
        x: position.x,
        y: position.y,
        resizable: false,
        decorations: false,
        transparent: true,
        alwaysOnTop: true,
        skipTaskbar: true,
        shadow: false,
        focus: false,
        focusable: false,
        visible: true
      })

      let failed = false
      bubble.once('tauri://error', (event) => {
        failed = true
        console.error('Failed to create floating bubble:', event.payload)
      })
      bindFloatingBubbleDestroyed(bubble)

      const visible = await waitForFloatingBubbleVisible(bubble)
      if (visible && !failed) {
        await correctFloatingBubbleSize(bubble)
        await reinforceFloatingBubbleTopmost()
      }
      return visible && !failed
    } catch (e) {
      console.error('Failed to show floating bubble:', e)
      return false
    }
  }

  async function closeFloatingBubble() {
    try {
      const bubble = await WebviewWindow.getByLabel(FLOATING_BUBBLE_LABEL)
      if (bubble) {
        closingFloatingBubbleInternally = true
        await invoke('clear_floating_bubble_topmost').catch(() => undefined)
        await bubble.close()
        window.setTimeout(() => {
          closingFloatingBubbleInternally = false
        }, 1000)
      }
    } catch (e) {
      closingFloatingBubbleInternally = false
      console.error('Failed to close floating bubble:', e)
    }
  }

  /**
   * 生成当前屏幕配置的唯一标识
   * 格式：{显示器数量}_{分辨率1}@{缩放1}_{分辨率2}@{缩放2}...
   * 按分辨率排序确保一致性
   */
  async function generateScreenConfigId(): Promise<string> {
    try {
      const monitors = await availableMonitors()
      if (monitors.length === 0) {
        return 'unknown'
      }

      // 收集所有显示器信息
      const monitorInfos: MonitorInfo[] = monitors.map(m => ({
        width: m.size.width,
        height: m.size.height,
        scaleFactor: Math.round(m.scaleFactor * 100) // 转换为百分比整数
      }))

      // 按分辨率排序（降序，大屏在前）
      monitorInfos.sort((a, b) => {
        const aPixels = a.width * a.height
        const bPixels = b.width * b.height
        return bPixels - aPixels
      })

      // 生成标识字符串
      const parts = monitorInfos.map(m => `${m.width}x${m.height}@${m.scaleFactor}`)
      return `${monitors.length}_${parts.join('_')}`
    } catch (e) {
      console.error('Failed to generate screen config id:', e)
      return 'unknown'
    }
  }

  /**
   * 生成人类可读的屏幕配置描述
   */
  function generateScreenConfigDisplayName(configId: string): string {
    if (configId === 'unknown' || configId === 'legacy') {
      return configId === 'legacy' ? '旧版配置' : '未知配置'
    }
    
    const parts = configId.split('_')
    const count = parts[0]
    const monitors = parts.slice(1).map(p => {
      const [res, scale] = p.split('@')
      return `${res} (${scale}%)`
    })
    
    return `${count}屏: ${monitors.join(' + ')}`
  }

  async function isWindowRectUsable(position: WindowPosition, size: WindowSize) {
    if (size.width < 320 || size.height < 400) return false

    const monitors = await availableMonitors()
    if (monitors.length === 0) return true

    const left = position.x
    const top = position.y
    const right = position.x + size.width
    const bottom = position.y + size.height

    return monitors.some((monitor) => {
      const monitorLeft = monitor.position.x
      const monitorTop = monitor.position.y
      const monitorRight = monitor.position.x + monitor.size.width
      const monitorBottom = monitor.position.y + monitor.size.height

      const overlapWidth = Math.min(right, monitorRight) - Math.max(left, monitorLeft)
      const overlapHeight = Math.min(bottom, monitorBottom) - Math.max(top, monitorTop)
      return overlapWidth >= 120 && overlapHeight >= 120
    })
  }

  async function centerMainWindowOnPrimary() {
    const monitor = await primaryMonitor()
    if (!monitor) return null

    const defaultWidth = 380
    const defaultHeight = 600
    const scale = monitor.scaleFactor
    const width = Math.round(defaultWidth * scale)
    const height = Math.round(defaultHeight * scale)
    const centerX = Math.round(monitor.position.x + (monitor.size.width - width) / 2)
    const centerY = Math.round(monitor.position.y + (monitor.size.height - height) / 2)

    await appWindow.setPosition(new PhysicalPosition(centerX, centerY))
    await appWindow.setSize(new PhysicalSize(width, height))

    return {
      position: { x: centerX, y: centerY },
      size: { width, height }
    }
  }

  // 初始化应用设置
  async function initSettings() {
    try {
      applyPlatformClass()
      await loadAppVersion()
      await listenFloatingBubbleStateChanges()
      
      // 生成当前屏幕配置标识
      currentScreenConfigId.value = await generateScreenConfigId()
      console.log('Current screen config ID:', currentScreenConfigId.value)

      // 保留旧版设置读取，避免历史配置缺失
      await loadAutoHideEnabled()

      // 加载深色主题设置
      await loadDarkTheme()
      
      // 尝试获取当前屏幕配置的保存记录
      const savedConfig = await invoke<ScreenConfig | null>('get_screen_config', {
        configId: currentScreenConfigId.value
      })
      
      const savedConfigUsable = savedConfig
        ? await isWindowRectUsable(
            { x: savedConfig.windowX, y: savedConfig.windowY },
            { width: savedConfig.windowWidth, height: savedConfig.windowHeight }
          )
        : false

      if (savedConfig && savedConfigUsable) {
        // 有保存的配置，恢复窗口状态
        console.log('Restoring saved screen config:', savedConfig)
        const floatingEnabled = await getPersistedFloatingBubbleEnabled(savedConfig.isFixed)
        
        setFloatingBubbleStateLocal(floatingEnabled)
        windowPosition.value = { x: savedConfig.windowX, y: savedConfig.windowY }
        windowSize.value = { width: savedConfig.windowWidth, height: savedConfig.windowHeight }
        
        // 恢复窗口位置
        try {
          await appWindow.setPosition(
            new PhysicalPosition(savedConfig.windowX, savedConfig.windowY)
          )
        } catch (e) {
          console.error('Failed to restore window position:', e)
        }
        
        // 恢复窗口尺寸
        try {
          await appWindow.setSize(
            new PhysicalSize(savedConfig.windowWidth, savedConfig.windowHeight)
          )
        } catch (e) {
          console.error('Failed to restore window size:', e)
        }
        
        // 如果已开启悬浮球入口，则恢复悬浮球
        if (floatingEnabled) {
          await applyFixedMode()
        }
        await syncFloatingBubbleChecked(floatingEnabled)
      } else {
        // 没有保存的配置，或保存的是隐藏/离屏产生的坏配置，则使用主屏幕中心位置。
        console.log(savedConfig ? 'Saved config is offscreen, using primary monitor center' : 'No saved config found, using primary monitor center')
        const floatingEnabled = await getPersistedFloatingBubbleEnabled(false)
        
        setFloatingBubbleStateLocal(floatingEnabled)
        
        try {
          const centered = await centerMainWindowOnPrimary()
          if (centered) {
            windowPosition.value = centered.position
            windowSize.value = centered.size
          }
        } catch (e) {
          console.error('Failed to center window:', e)
        }
        
        // 为当前配置创建初始记录
        await saveWindowState()
      }
      
      // 加载所有屏幕配置列表
      await loadScreenConfigs()
      
      // 加载日历显示状态
      await loadShowCalendar()
    } catch (e) {
      console.error('Failed to load settings:', e)
    }
  }

  // 加载日历显示状态
  async function loadShowCalendar() {
    try {
      showCalendar.value = await invoke<boolean>('get_show_calendar')
    } catch (e) {
      console.error('Failed to load show calendar setting:', e)
      showCalendar.value = false
    }
  }

  // 加载贴边自动隐藏设置
  async function loadAutoHideEnabled() {
    try {
      autoHideEnabled.value = await invoke<boolean>('get_auto_hide_enabled')
    } catch (e) {
      console.error('Failed to load auto hide setting:', e)
      autoHideEnabled.value = true
    }
  }

  // 切换日历显示
  async function toggleShowCalendar() {
    try {
      showCalendar.value = !showCalendar.value
      await invoke('set_show_calendar', { show: showCalendar.value })
    } catch (e) {
      console.error('Failed to toggle show calendar:', e)
      showCalendar.value = !showCalendar.value // 回滚
    }
  }

  // 设置日历显示
  async function setShowCalendar(show: boolean) {
    try {
      showCalendar.value = show
      await invoke('set_show_calendar', { show })
    } catch (e) {
      console.error('Failed to set show calendar:', e)
    }
  }

  // 设置贴边自动隐藏
  async function setAutoHideEnabled(enabled: boolean) {
    const oldValue = autoHideEnabled.value
    try {
      autoHideEnabled.value = enabled
      await invoke('set_auto_hide_enabled', { enabled })
      await saveWindowState()
    } catch (e) {
      console.error('Failed to set auto hide enabled:', e)
      autoHideEnabled.value = oldValue
    }
  }

  // 设置悬浮球入口开关。沿用旧 isFixed 字段持久化，避免破坏历史数据。
  async function setFloatingBubbleEnabled(enabled: boolean) {
    const oldValue = isFixed.value
    const oldMode = windowMode.value

    try {
      setFloatingBubbleStateLocal(enabled)

      if (isFixed.value) {
        await applyFixedMode()
      } else {
        await applyNormalMode()
      }

      await syncFloatingBubbleChecked(isFixed.value)
      if (appWindow.label === 'main') {
        await saveWindowState()
      } else {
        await saveFloatingBubbleSetting(isFixed.value)
      }
      await broadcastFloatingBubbleState(isFixed.value)
    } catch (e) {
      console.error('Failed to set floating bubble:', e)
      setFloatingBubbleStateLocal(oldValue)
      windowMode.value = oldMode
      await syncFloatingBubbleChecked(oldValue)
      await broadcastFloatingBubbleState(oldValue)
    }
  }

  // 切换悬浮球入口（旧方法名保留给托盘和现有调用）
  async function toggleFixedMode() {
    await setFloatingBubbleEnabled(!isFixed.value)
  }

  // 应用悬浮球入口：只显示右侧圆形入口，不隐藏主窗口，不影响新建待办按钮
  async function applyFixedMode() {
    try {
      const bubbleReady = await showFloatingBubble()
      if (!bubbleReady) {
        setFloatingBubbleStateLocal(false)
        await syncFloatingBubbleChecked(false)
        throw new Error('悬浮球创建失败')
      }
    } catch (e) {
      console.error('Failed to show floating bubble:', e)
      setFloatingBubbleStateLocal(false)
      await closeFloatingBubble()
      throw e
    }
  }

  // 关闭悬浮球入口；如果主窗口之前被悬浮球隐藏，则顺手恢复主窗口
  async function applyNormalMode() {
    try {
      await closeFloatingBubble()
      await showMainWindow()
    } catch (e) {
      console.error('Failed to close floating bubble:', e)
    }
  }

  // 加载深色主题设置
  async function loadDarkTheme() {
    try {
      const settings = await invoke<{ textTheme: string }>('get_settings')
      isDarkTheme.value = settings.textTheme === 'light'
      applyThemeClass()
    } catch (e) {
      console.error('Failed to load dark theme setting:', e)
      isDarkTheme.value = false
    }
  }

  // 切换深色主题
  async function toggleDarkTheme() {
    isDarkTheme.value = !isDarkTheme.value
    applyThemeClass()
    await saveWindowState()
  }

  // 应用主题 CSS class
  function applyThemeClass() {
    document.body.classList.toggle('dark-theme', isDarkTheme.value)
  }

  // 保存窗口状态（位置和尺寸）到当前屏幕配置
  async function saveWindowState() {
    try {
      if (appWindow.label !== 'main') return

      const visible = await appWindow.isVisible().catch(() => true)
      if (!visible) return

      const persistState = await invoke<WindowPersistState>('get_window_persist_state')
      const position = persistState.position
      const size = persistState.size

      if (!await isWindowRectUsable(position, size)) {
        console.warn('Skip saving unusable main window state:', { position, size })
        return
      }

      windowPosition.value = { x: position.x, y: position.y }
      windowSize.value = { width: size.width, height: size.height }
      
      // 确保有当前屏幕配置 ID
      if (!currentScreenConfigId.value) {
        currentScreenConfigId.value = await generateScreenConfigId()
      }
      
      // 保存到屏幕配置表
      const configRequest: SaveScreenConfigRequest = {
        configId: currentScreenConfigId.value,
        displayName: generateScreenConfigDisplayName(currentScreenConfigId.value),
        windowX: position.x,
        windowY: position.y,
        windowWidth: Math.round(size.width),
        windowHeight: Math.round(size.height),
        isFixed: isFixed.value
      }
      
      await invoke('save_screen_config', { config: configRequest })
      
      await invoke('save_settings', {
        settings: {
          isFixed: isFixed.value,
          windowPosition: windowPosition.value,
          windowSize: windowSize.value,
          autoHideEnabled: autoHideEnabled.value,
          textTheme: isDarkTheme.value ? 'light' : 'dark'
        }
      })
    } catch (e) {
      console.error('Failed to save window state:', e)
    }
  }

  // 加载所有屏幕配置列表
  async function loadScreenConfigs() {
    try {
      screenConfigs.value = await invoke<ScreenConfig[]>('list_screen_configs')
    } catch (e) {
      console.error('Failed to load screen configs:', e)
      screenConfigs.value = []
    }
  }

  // 删除屏幕配置
  async function deleteScreenConfig(configId: string): Promise<boolean> {
    try {
      await invoke('delete_screen_config', { configId })
      await loadScreenConfigs()
      return true
    } catch (e) {
      console.error('Failed to delete screen config:', e)
      return false
    }
  }

  // 更新屏幕配置名称
  async function updateScreenConfigName(configId: string, displayName: string): Promise<boolean> {
    try {
      await invoke('update_screen_config_name', { configId, displayName })
      await loadScreenConfigs()
      return true
    } catch (e) {
      console.error('Failed to update screen config name:', e)
      return false
    }
  }

  // 导出数据
  async function exportData(): Promise<string | null> {
    try {
      return await invoke<string>('export_data')
    } catch (e) {
      console.error('Failed to export data:', e)
      return null
    }
  }

  // 导入数据
  async function importData(jsonData: string): Promise<boolean> {
    try {
      await invoke('import_data', { jsonData })
      return true
    } catch (e) {
      console.error('Failed to import data:', e)
      return false
    }
  }

  // 比较版本号 (返回: 1 表示 v1 > v2, -1 表示 v1 < v2, 0 表示相等)
  function compareVersions(v1: string, v2: string): number {
    const parts1 = v1.replace(/^v/, '').split('.').map(Number)
    const parts2 = v2.replace(/^v/, '').split('.').map(Number)
    
    for (let i = 0; i < Math.max(parts1.length, parts2.length); i++) {
      const p1 = parts1[i] || 0
      const p2 = parts2[i] || 0
      if (p1 > p2) return 1
      if (p1 < p2) return -1
    }
    return 0
  }

  // 检查版本更新
  async function checkForUpdates(): Promise<boolean> {
    try {
      hasUpdate.value = false
      latestVersion.value = null
      releaseUrl.value = null
      updateInstallerAsset.value = null
      updateCheckError.value = null

      await loadAppVersion()
      if (!APP_VERSION.value) {
        console.log('App version unavailable; skip update check')
        return false
      }

      const release = await invoke<LatestReleaseInfo>('get_latest_release_info')
      const tagName = release.tagName
      
      // 比较版本号
      if (compareVersions(tagName, APP_VERSION.value) > 0) {
        hasUpdate.value = true
        latestVersion.value = tagName
        releaseUrl.value = release.releaseUrl || GITHUB_RELEASES_URL
        updateInstallerAsset.value = release.installerAsset
      }
      return true
    } catch (e) {
      console.error('Failed to check for updates:', e)
      updateCheckError.value = String(e)
      return false
    }
  }

  // 获取 GitHub Release 页面 URL
  function getReleasesUrl(): string {
    return releaseUrl.value || GITHUB_RELEASES_URL
  }

  function getUpdateInstallerAsset(): UpdateAsset | null {
    return updateInstallerAsset.value
  }

  async function downloadUpdateInstaller(): Promise<UpdateDownloadResult> {
    const asset = updateInstallerAsset.value
    if (!asset) {
      throw new Error('当前平台没有可自动安装的更新包')
    }

    return await invoke<UpdateDownloadResult>('download_update_installer', {
      downloadUrl: asset.downloadUrl,
      fileName: asset.name
    })
  }

  async function installUpdateAndExit(installerPath: string) {
    await invoke('install_update_and_exit', { installerPath })
  }

  return {
    // 状态
    isFixed,
    isDarkTheme,
    windowPosition,
    windowSize,
    windowMode,
    hasUpdate,
    latestVersion,
    updateCheckError,
    // 屏幕配置状态
    currentScreenConfigId,
    screenConfigs,
    // 日历状态
    showCalendar,
    autoHideEnabled,
    // 方法
    initSettings,
    loadAppVersion,
    listenFloatingBubbleStateChanges,
    toggleFixedMode,
    setFloatingBubbleEnabled,
    loadFloatingBubbleEnabled,
    toggleDarkTheme,
    saveWindowState,
    exportData,
    importData,
    checkForUpdates,
    getReleasesUrl,
    getUpdateInstallerAsset,
    downloadUpdateInstaller,
    installUpdateAndExit,
    // 屏幕配置方法
    generateScreenConfigId,
    generateScreenConfigDisplayName,
    loadScreenConfigs,
    deleteScreenConfig,
    updateScreenConfigName,
    // 日历方法
    loadShowCalendar,
    toggleShowCalendar,
    setShowCalendar,
    // 自动隐藏方法
    loadAutoHideEnabled,
    setAutoHideEnabled
  }
})
