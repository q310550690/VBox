<!-- 播放器窗格（详情页内嵌）：16:9 + 状态浮层 + hover 显隐控制条 + 窗格全屏
     - 视频由 libmpv 原生子窗口渲染（位于 WebView2 下层）：播放/暂停/缓冲/结束时，
       .app-shell 会用 evenodd clip-path 挖出视频洞（见 layouts/default），窗格本身完全透明；
       洞内不能有任何 HTML 绘制，否则挡住视频。
     - 洞开启时，交互层 / 控制条 / 标题用 <Teleport to="body"> 渲染成 fixed 浮层（位于洞上层），
       坐标复用 usePlayer 上报的同一份 CSS rect；视频不可见时（idle/opening/error/demo）不 teleport。
     - 原生窗口是矩形且不能圆角裁剪：播放态该区域方角，外框用 1px 描边处理。
     - 浏览器演示模式没有原生视频：展示占位说明，不伪造播放画面。 -->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import SolarIcon from '@/components/SolarIcon.vue'
import PlayerControls from './PlayerControls.vue'
import PlayerTopBar from './PlayerTopBar.vue'
import SubtitleSearchDialog from './SubtitleSearchDialog.vue'
import { usePlayer } from '@/composables/usePlayer'
import {
  danmakuFetch,
  formatError,
  isTauri,
  playerAddSubtitle,
  playerPickSubtitle,
  playerPrefs,
  playerSaveCurrent,
  playerSavePause,
  playerSaveResume,
  playerSetAudioTrack,
  playerSetEnhance,
  playerSetSubtitleTrack,
  pluginOverview,
  pluginSetEnabled,
  pluginSetPreset,
} from '@/api'
import type { PluginInfo } from '@/types'
import { useUiStore } from '@/store/ui'
import { useDownloadsStore } from '@/store/downloads'
import { formatDuration } from '@/utils/format'
import { explainMediaError } from '@/utils/mediaError'
import type { DownloadFileInfo, Episode, PlayerState, PlayerTrack } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 单集标题（idle/opening 浮层展示） */
    title?: string
    /** 剧集名称（顶部信息条左上角） */
    seriesName?: string
    /** 当前集文案，如「第 3 集」 */
    episodeLabel?: string
    /** 当前线路的选集（底部「选集」弹层 + 上/下一集可用性） */
    episodes?: Episode[]
    /** 当前集在线路内的下标（-1 = 未选中） */
    activeEpisode?: number
    /** 可选字幕轨道（后端未接入时为空） */
    subtitleTracks?: PlayerTrack[]
    /** 可选音轨（后端未接入时为空） */
    audioTracks?: PlayerTrack[]
    /** 当前字幕轨 id */
    activeSubtitle?: string
    /** 当前音轨 id */
    activeAudio?: string
    /** 片头/片尾标记的存储键（一般传 siteKey/videoId；缺省时用剧集名） */
    markerKey?: string
    /** 封面（idle 态模糊背景 + 演示模式占位） */
    cover?: string | null
    /** 是否已选中剧集 / 已有媒体（false = idle「选择剧集开始播放」） */
    hasMedia?: boolean
    /** 是否正在解析（显示 opening 态文案；不伪造播放） */
    loading?: boolean
    /** opening 态文案（解析 / 打开播放器） */
    loadingText?: string
    /** 无媒体时的错误文案（解析失败），显示 error 态 + 重试 */
    errorText?: string
    /** 单片「边下边播」开关状态（详情页算好：本片覆盖优先，否则跟随全局设置） */
    autoSave?: boolean
    /** 当前播放集的站点 / 线路 / 影片 / 集信息；用于「保存本集」计算下载目录 */
    saveInfo?: DownloadFileInfo | null
    /** 独立播放器窗口模式：窗格铺满整个窗口（无 16:9 / 圆角），并显示关闭按钮与 HDR 标识 */
    windowMode?: boolean
  }>(),
  {
    title: '',
    seriesName: '',
    episodeLabel: '',
    episodes: () => [],
    activeEpisode: -1,
    subtitleTracks: () => [],
    audioTracks: () => [],
    activeSubtitle: '',
    activeAudio: '',
    markerKey: '',
    cover: null,
    hasMedia: false,
    loading: false,
    loadingText: '',
    errorText: '',
    autoSave: false,
    saveInfo: null,
    windowMode: false,
  },
)

const emit = defineEmits<{
  (e: 'retry'): void
  (e: 'prev'): void
  (e: 'next'): void
  (e: 'select-episode', index: number): void
  (e: 'toggle-auto-save'): void
  /** 播放器进入 error 态（详情页据此做「复用缓存地址失效 → 重新解析」回退） */
  (e: 'error', message: string): void
  /** 控制条「弹窗播放」：交给详情页切到独立播放器窗口 */
  (e: 'pop-out'): void
  /** 独立播放器窗口模式：用户点击右上角关闭按钮 */
  (e: 'close-window'): void
}>()

const ui = useUiStore()
const downloads = useDownloadsStore()

/** 内部使用 usePlayer：状态/命令/视频洞矩形的唯一来源；页面通过 defineExpose 调用 open/close */
const {
  state: playerState,
  videoRect,
  open: playerOpen,
  close,
  play,
  togglePause,
  seek,
  setVolume,
  setMute,
  setRate,
  setVisible,
  syncRect,
  stopRectSync,
} = usePlayer()

/** 播放器窗格（原生窗口 rect 上报目标） */
const stageRef = ref<HTMLElement | null>(null)

/* ---------------- 全屏 / 小窗 ---------------- */

const fullscreen = ref(false)
/** 小窗播放：窗格 fixed 到右下角，原生视频洞跟随（详情页需容忍列高度塌陷） */
const mini = ref(false)

async function afterLayoutChange() {
  await nextTick()
  // fixed / 全屏切换后位置变化不一定触发 ResizeObserver：手动重算一次
  syncRect(stageRef.value)
}

/* ---- 真·全屏：把 Tauri 主窗口切到系统全屏（铺满当前显示器），并隐藏应用外壳 ---- */

/** 全屏切换请求进行中（防止连点导致窗口状态与本地状态错位） */
let fullscreenBusy = false
/** 组件已卸载：不再补发 rect 同步（卸载时 fullscreen 复位会触发 watcher） */
let paneDisposed = false
/** 全屏后窗口尺寸会分几帧才稳定：补发几次 rect 同步，避免 mpv 视频洞错位 */
let fullscreenSyncTimers: number[] = []

function clearFullscreenSyncTimers() {
  for (const id of fullscreenSyncTimers) window.clearTimeout(id)
  fullscreenSyncTimers = []
}

function scheduleFullscreenSync() {
  if (paneDisposed) return
  clearFullscreenSyncTimers()
  const sync = () => {
    if (!paneDisposed) syncRect(stageRef.value)
  }
  sync()
  for (const delay of [80, 240, 600]) {
    fullscreenSyncTimers.push(window.setTimeout(sync, delay))
  }
}

/** 外壳隐藏标记：用 <html data-player-fullscreen> 而不是 class，避免被主题重写 className 抹掉 */
function applyFullscreenChrome(on: boolean) {
  const root = document.documentElement
  if (on) root.setAttribute('data-player-fullscreen', '')
  else root.removeAttribute('data-player-fullscreen')
}

/* ---------------- 视频表面接管 / 外壳底色 ---------------- */

/**
 * mpv 用 `--wid` 直接渲染在主窗口表面：第一次真正起播后，窗口内容被视频表面接管，
 * 半透明亚克力外壳会透出视频表面的黑色（亮色主题下侧栏/标题栏会变成深灰）。
 * 这里给 <html> 打标，让外壳改用不透明主题色（规则见 base.css 的 .player-shell-solid）。
 * 实测关闭播放 / stop 不会让窗口表面恢复透明，因此本会话内保持不透明；
 * 重启应用后窗口表面重建，亚克力自然恢复。
 */
function markSurfaceOwned() {
  if (!isTauri) return
  document.documentElement.classList.add('player-shell-solid')
}

/** 调用 Tauri 窗口 API 切换系统全屏；失败时退回窗格 CSS 全屏（不影响播放本身） */
async function setWindowFullscreen(on: boolean) {
  if (!isTauri) return
  try {
    // 浏览器预览下 getCurrentWindow() 会因缺少 __TAURI_INTERNALS__ 抛错，上面已用 isTauri 拦住
    await getCurrentWindow().setFullscreen(on)
  } catch (e) {
    console.warn('[VBox] 切换系统全屏失败：', e)
    ui.notify(on ? '切换系统全屏失败，已使用播放器内全屏' : '退出系统全屏失败', 'error')
  }
}

/**
 * 全屏切换：
 * - Tauri 桌面端：主窗口 setFullscreen(true) 真正铺满当前显示器（原来只是 CSS class，
 *   窗口本身仍留在详情页大小，所以「没有做到显示器全屏」）；
 * - 全屏期间隐藏标题栏 / 侧栏等外壳，只保留播放器与控制条浮层；
 * - 窗口尺寸/布局变化后重算视频洞 rect（mpv 以 --wid 画在主窗口上，margins 必须跟着 rect 走）。
 */
async function toggleFullscreen() {
  if (fullscreenBusy) return
  fullscreenBusy = true
  const next = !fullscreen.value
  try {
    if (next) mini.value = false
    await setWindowFullscreen(next)
    fullscreen.value = next
    applyFullscreenChrome(next)
    // 进出全屏先亮出控制条，之后由「鼠标静止 3s」计时自动收起（全屏下同样生效）
    if (next) {
      showControls.value = true
      scheduleHide()
    }
    await afterLayoutChange()
    scheduleFullscreenSync()
  } finally {
    fullscreenBusy = false
  }
}

async function toggleMini() {
  if (fullscreen.value || props.windowMode) return
  mini.value = !mini.value
  await afterLayoutChange()
}

/** 焦点在输入类元素时不要抢按键（搜索框、设置项等） */
function isTextInputTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return (
    tag === 'INPUT' ||
    tag === 'TEXTAREA' ||
    tag === 'SELECT' ||
    el.isContentEditable ||
    Boolean(el.closest('[contenteditable="true"]'))
  )
}

/** 焦点在按钮/链接上时空格交给浏览器原生点击，避免播 / 暂停触发两次 */
function isClickableTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  return ['BUTTON', 'A', 'SUMMARY'].includes(el.tagName) || el.getAttribute('role') === 'button'
}

function flashControls() {
  showControls.value = true
  scheduleHide()
}

function onKeydown(event: KeyboardEvent) {
  if (event.ctrlKey || event.metaKey || event.altKey) return
  if (isTextInputTarget(event.target)) return

  if (event.key === 'Escape') {
    if (fullscreen.value) {
      event.preventDefault()
      void toggleFullscreen()
      return
    }
    if (mini.value) {
      event.preventDefault()
      void toggleMini()
    }
    return
  }

  // 只有真正有媒体时才接管快捷键，避免在空播放器/加载页吞掉页面按键
  if (!mediaActive.value) return

  switch (event.key) {
    case ' ':
    case 'k':
    case 'K': {
      if (event.repeat) return
      if (event.key === ' ' && isClickableTarget(event.target)) return
      event.preventDefault()
      flashControls()
      void togglePause()
      break
    }
    case 'ArrowLeft':
    case 'ArrowRight': {
      event.preventDefault()
      const step = event.shiftKey ? 30 : 5
      const dir = event.key === 'ArrowRight' ? 1 : -1
      const duration = playerState.value.durationSec || 0
      const target = playerState.value.positionSec + dir * step
      const clamped = duration > 0 ? Math.min(duration, Math.max(0, target)) : Math.max(0, target)
      flashControls()
      void seek(clamped)
      break
    }
    case 'ArrowUp':
    case 'ArrowDown': {
      event.preventDefault()
      const dir = event.key === 'ArrowUp' ? 1 : -1
      const next = Math.min(100, Math.max(0, Math.round(playerState.value.volume) + dir * 5))
      flashControls()
      void setVolume(next)
      break
    }
    case 'f':
    case 'F':
      event.preventDefault()
      flashControls()
      void toggleFullscreen()
      break
    case 'm':
    case 'M':
      event.preventDefault()
      flashControls()
      void setMute(!playerState.value.muted)
      break
    default:
      break
  }
}

onMounted(() => {
  syncRect(stageRef.value)
  window.addEventListener('keydown', onKeydown)
  void setVisible(mediaActive.value)
  void loadEnhance()
  void loadPlugins()
})

onBeforeUnmount(() => {
  paneDisposed = true
  window.removeEventListener('keydown', onKeydown)
  clearHideTimer()
  clearClickTimer()
  // 离开播放页时务必退出系统全屏并恢复外壳，避免把整个应用卡在无标题栏的全屏态
  if (fullscreen.value) {
    fullscreen.value = false
    applyFullscreenChrome(false)
    void setWindowFullscreen(false)
  }
  void setVisible(false)
  stopRectSync()
  clearFullscreenSyncTimers()
})

/* ---------------- 本地偏好（循环 / 弹幕 / 片头片尾标记） ---------------- */

const PREF_LOOP = 'vbox.player.loop'
const PREF_DANMAKU = 'vbox.player.danmaku'
const MARKER_STORAGE = 'vbox.player.markers'

/** 片头/片尾标记集合：{ [markerKey]: { intro, outro } }（秒） */
interface MarkerSet {
  intro?: number
  outro?: number
}

function readBoolPref(key: string, fallback: boolean): boolean {
  try {
    const raw = localStorage.getItem(key)
    if (raw === null) return fallback
    return raw === '1'
  } catch {
    return fallback
  }
}

function writeBoolPref(key: string, value: boolean) {
  try {
    localStorage.setItem(key, value ? '1' : '0')
  } catch {
    // localStorage 不可用（隐私模式等）：仅内存态
  }
}

function readMarkerStore(): Record<string, MarkerSet> {
  try {
    const raw = localStorage.getItem(MARKER_STORAGE)
    const parsed = raw ? (JSON.parse(raw) as unknown) : null
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, MarkerSet>) : {}
  } catch {
    return {}
  }
}

function writeMarkerStore(store: Record<string, MarkerSet>) {
  try {
    localStorage.setItem(MARKER_STORAGE, JSON.stringify(store))
  } catch {
    // 同上：忽略持久化失败
  }
}

const loop = ref(readBoolPref(PREF_LOOP, false))
const danmaku = ref(readBoolPref(PREF_DANMAKU, false))
/** 画质增强档位（Anime4K 超分）：持久化在 player.json，由后端在 player_open 时应用 */
const enhance = ref('off')
const introSec = ref<number | null>(null)
const outroSec = ref<number | null>(null)
/** 片尾只触发一次（重新打开媒体时复位） */
let outroFired = false

watch(loop, (value) => writeBoolPref(PREF_LOOP, value))
watch(danmaku, (value) => writeBoolPref(PREF_DANMAKU, value))

/** 读取播放器页面「画质」开关的当前档位（内置插件状态为事实来源；插件未加载时回退 player.json） */
let enhanceFromPlugins = false
async function loadEnhance() {
  try {
    const prefs = await playerPrefs()
    if (!enhanceFromPlugins) enhance.value = prefs.enhance || 'off'
  } catch {
    if (!enhanceFromPlugins) enhance.value = 'off'
  }
}

/** 切换画质增强：驱动内置 Anime4K 插件，并写回 player.json */
async function onEnhance(mode: string) {
  const previous = enhance.value
  enhance.value = mode
  try {
    enhance.value = await playerSetEnhance(mode)
    void loadPlugins()
    if (mode !== 'off') ui.notify('画质增强已切换（GPU 较弱时可能掉帧）', 'info')
  } catch (e) {
    enhance.value = previous
    ui.notify(`画质增强切换失败：${formatError(e)}`, 'error')
  }
}

/** 已安装插件（着色器 / 滤镜）：画质浮层里做快速启停，详细管理在设置页 */
const pluginItems = ref<PluginInfo[]>([])
const BUNDLED_PLUGIN_ID = 'vbox.anime4k'
const ENHANCE_PRESETS = ['lite', 'standard', 'strong']

/** 内置 Anime4K 已变成普通插件：四档「画质」由它的启用状态 + 预设反推。 */
function syncEnhanceFromPlugins() {
  const bundled = pluginItems.value.find((plugin) => plugin.id === BUNDLED_PLUGIN_ID)
  if (!bundled) return
  enhanceFromPlugins = true
  const preset = bundled.preset || 'standard'
  enhance.value =
    bundled.enabled && ENHANCE_PRESETS.includes(preset) ? preset : 'off'
}

async function loadPlugins() {
  try {
    const overview = await pluginOverview()
    // 安全模式下后端不加载插件，浮层也不展示，避免误导为「已生效」
    pluginItems.value = overview.disabled ? [] : overview.plugins
    if (!overview.disabled) syncEnhanceFromPlugins()
  } catch {
    pluginItems.value = []
  }
}

async function onSelectPluginPreset(id: string, preset: string) {
  try {
    const plugin = pluginItems.value.find((item) => item.id === id)
    const name = plugin?.name || id
    // 二级菜单里的「关闭」：停用插件，而不是把预设写成空值。
    if (!preset) {
      if (plugin?.enabled) await pluginSetEnabled(id, false)
      await loadPlugins()
      ui.notify(`${name} 已关闭`, 'info')
      return
    }
    await pluginSetPreset(id, preset)
    if (!plugin?.enabled) await pluginSetEnabled(id, true)
    await loadPlugins()
    const label = plugin?.shaders.find((shader) => shader.id === preset)?.name || preset
    ui.notify(`${name} 已切换为「${label}」`, 'info')
  } catch (e) {
    ui.notify(`插件预设切换失败：${formatError(e)}`, 'error')
  }
}

async function onTogglePlugin(id: string, enabled: boolean) {
  try {
    const overview = await pluginSetEnabled(id, enabled)
    pluginItems.value = overview.disabled ? [] : overview.plugins
    if (!overview.disabled) syncEnhanceFromPlugins()
    ui.notify(enabled ? '插件已启用' : '插件已停用', 'info')
  } catch (e) {
    ui.notify(`插件切换失败：${formatError(e)}`, 'error')
  }
}

/** 当前标记键：优先 markerKey，退化到剧集名 / 单集标题 */
const markerId = computed(() => props.markerKey || props.seriesName || props.title || '')

function loadMarkers() {
  const id = markerId.value
  if (!id) {
    introSec.value = null
    outroSec.value = null
    return
  }
  const set = readMarkerStore()[id]
  introSec.value = typeof set?.intro === 'number' ? set.intro : null
  outroSec.value = typeof set?.outro === 'number' ? set.outro : null
}

watch(markerId, loadMarkers, { immediate: true })

function persistMarkers() {
  const id = markerId.value
  if (!id) return
  const store = readMarkerStore()
  const next: MarkerSet = {}
  if (typeof introSec.value === 'number') next.intro = introSec.value
  if (typeof outroSec.value === 'number') next.outro = outroSec.value
  if (next.intro === undefined && next.outro === undefined) delete store[id]
  else store[id] = next
  writeMarkerStore(store)
}

const currentPosition = computed(() => Math.max(0, Math.round(playerState.value.positionSec)))

function setIntro() {
  introSec.value = currentPosition.value
  persistMarkers()
  ui.notify(`片头已设为 ${formatDuration(currentPosition.value)}`, 'success')
}

function clearIntro() {
  introSec.value = null
  persistMarkers()
  ui.notify('已清除片头标记', 'info')
}

function setOutro() {
  outroSec.value = currentPosition.value
  outroFired = false
  persistMarkers()
  ui.notify(`片尾已设为 ${formatDuration(currentPosition.value)}`, 'success')
}

function clearOutro() {
  outroSec.value = null
  outroFired = false
  persistMarkers()
  ui.notify('已清除片尾标记', 'info')
}

/* ---------------- 播放命令（面板内闭环） ---------------- */

/** 重播：回到 0 秒并继续播放 */
async function replay() {
  await seek(0)
  await play()
}

async function toggleLoop() {
  loop.value = !loop.value
  ui.notify(loop.value ? '已开启循环播放' : '已关闭循环播放', 'info')
}

/* ---------------- 弹幕（B 站 → ASS 外挂字幕） ---------------- */

/** 已加载弹幕对应的播放地址（切集 / 重开时用于避免重复加载） */
let danmakuLoadedFor = ''

/** 若弹幕开关为开，则为当前播放地址拉取弹幕并加载为外挂字幕 */
async function loadDanmakuForCurrent() {
  if (!danmaku.value || !isTauri) return
  const url = playerState.value.url
  if (!url || danmakuLoadedFor === url) return
  try {
    const path = await danmakuFetch(url, props.seriesName.trim() || props.title.trim())
    if (!path) {
      danmakuLoadedFor = url
      ui.notify('当前视频未找到弹幕', 'info')
      return
    }
    danmakuLoadedFor = url
    await playerAddSubtitle(path)
    ui.notify('弹幕已加载', 'success')
  } catch (e) {
    console.warn('[VBox] 弹幕加载失败：', e)
    ui.notify(`弹幕加载失败：${formatError(e)}`, 'error')
  }
}

function toggleDanmaku() {
  danmaku.value = !danmaku.value
  if (danmaku.value) {
    void loadDanmakuForCurrent()
  } else {
    danmakuLoadedFor = ''
    void playerSetSubtitleTrack('no')
  }
}

function onReplay() {
  void replay()
}

/** 打开媒体：应用片头标记（起始位置早于片头时直接跳到片头） */
async function open(url: string, headers?: Record<string, string>, startSec?: number) {
  outroFired = false
  // 起播后 mpv 接管主窗口表面：外壳改用不透明主题色，避免亚克力透出视频表面的黑色
  markSurfaceOwned()
  // 起播瞬间先亮出控制条，之后由鼠标静止计时自动收起
  showControls.value = true
  scheduleHide()
  const intro = introSec.value
  const start = startSec ?? 0
  await playerOpen(url, headers, intro !== null && start < intro ? intro : startSec)
  // 起播后按开关补拉弹幕（player-state 稍后回填 url 时由下面的 watch 兜底）
  void loadDanmakuForCurrent()
}

/* ---------------- 字幕 / 音轨（来自 mpv track-list 的播放器状态快照） ---------------- */

/** 轨道列表以 player-state 快照为准；页面传入的 props 仅作旧兼容（为空时不覆盖） */
const subtitleTracks = computed(() => playerState.value.subtitleTracks ?? [])
const audioTracks = computed(() => playerState.value.audioTracks ?? [])

/** 当前选中的轨道 id（空串 = 关闭/无选中，与 PlayerControls 的语义一致） */
const activeSubtitle = computed(() =>
  playerState.value.activeSubtitleId === 'no' ? '' : playerState.value.activeSubtitleId,
)
const activeAudio = computed(() =>
  playerState.value.activeAudioId === 'no' ? '' : playerState.value.activeAudioId,
)

/** 顶部信息条展示的当前音轨文案（名称 + 编码，关闭时显示「静音」） */
const activeAudioLabel = computed(() => {
  if (playerState.value.activeAudioId === 'no') return '静音'
  const track = audioTracks.value.find((t) => t.id === playerState.value.activeAudioId)
  if (!track) return ''
  return [track.label, track.codec?.toUpperCase()].filter(Boolean).join(' ')
})

/** 选择字幕轨：空串 = 关闭（后端 sid=no） */
async function onSelectSubtitle(id: string) {
  try {
    await playerSetSubtitleTrack(id || 'no')
  } catch (e) {
    console.warn('[VBox] 切换字幕轨失败：', e)
  }
}

/** 选择音轨：空串 = 关闭（后端 aid=no） */
async function onSelectAudio(id: string) {
  try {
    await playerSetAudioTrack(id || 'no')
  } catch (e) {
    console.warn('[VBox] 切换音轨失败：', e)
  }
}

/**
 * 加载本地外挂字幕：原生文件对话框选文件 → 后端 mpv `sub-add`（自动选中）。
 * 新轨由 `player-state` 事件带回 `subtitleTracks`，这里不手工维护轨道列表。
 */
async function onLoadSubtitle() {
  if (!isTauri) {
    ui.notify('浏览器预览无法加载本地字幕（需桌面端）', 'info')
    return
  }
  try {
    const path = await playerPickSubtitle()
    if (!path) return
    await playerAddSubtitle(path)
    ui.notify('字幕已加载', 'success')
  } catch (e) {
    ui.notify(`字幕加载失败：${formatError(e)}`, 'error')
  }
}

/* ---------------- 在线字幕搜索 ---------------- */

/** 在线字幕搜索弹窗显隐 */
const subtitleDialogOpen = ref(false)
/** 弹窗默认关键词：优先剧名，其次单集标题 */
const onlineSubtitleQuery = computed(() => props.seriesName.trim() || props.title.trim())

/** 打开在线字幕搜索弹窗 */
function onSearchOnlineSubtitle() {
  subtitleDialogOpen.value = true
}

/** 在线字幕下载完成：把本地路径加载进 mpv */
async function onOnlineSubtitleLoaded(path: string) {
  try {
    await playerAddSubtitle(path)
    ui.notify('在线字幕已加载', 'success')
  } catch (e) {
    ui.notify(`字幕加载失败：${formatError(e)}`, 'error')
  }
}

/* ---------------- 保存本集（HLS 边看边存） ---------------- */

/** 当前播放地址是否为网络流（HLS 代理 / mp4 直链 / DASH 等都可以保存） */
const canSaveEpisode = computed(() => /^https?:\/\//i.test(playerState.value.url))
/** 保存状态：idle → saving ⇄ paused → done */
type SaveState = 'idle' | 'saving' | 'paused' | 'done'
const saveState = ref<SaveState>('idle')
/** 切集后把「已保存」复位，避免上一集的完成图标带到下一集 */
watch(
  () => playerState.value.url,
  () => {
    if (saveState.value === 'done') saveState.value = 'idle'
  },
)

// 切集后自动刷新弹幕（开关为开时才请求）
watch(
  () => playerState.value.url,
  (url) => {
    if (!url) return
    danmakuLoadedFor = ''
    void loadDanmakuForCurrent()
  },
)

/**
 * 「保存本集」按钮：
 * - idle / done → 开始保存（refresh 图标旋转）；
 * - saving → 再次点击暂停；
 * - paused → 再次点击继续下载。
 */
async function onSaveEpisode() {
  if (saveState.value === 'saving') {
    if (await playerSavePause()) {
      saveState.value = 'paused'
      ui.notify('已暂停保存，点击继续', 'info')
    }
    return
  }
  if (saveState.value === 'paused') {
    if (await playerSaveResume()) {
      saveState.value = 'saving'
      ui.notify('继续保存…', 'info')
    }
    return
  }
  const seriesName = props.seriesName.trim() || props.title.trim() || 'HLS 缓存'
  const episode = props.episodeLabel.trim()
  // 详情页会传完整信息（站点 / 线路 / vodId / 集序号）；其他页面回退到占位站点/线路
  const info: DownloadFileInfo = props.saveInfo ?? {
    siteKey: '',
    siteName: '',
    line: '',
    vodId: '',
    epIndex: 0,
    name: seriesName,
    epTitle: episode,
    cover: props.cover ?? undefined,
  }
  saveState.value = 'saving'
  ui.notify('开始保存（缺失分片会自动补下，可点击暂停）…', 'info')
  try {
    const path = await playerSaveCurrent(info)
    saveState.value = 'done'
    ui.notify(`已保存：${path}`, 'success')
    // 保存结果会登记为一条已完成下载记录，刷新下载列表方便用户直接去下载页播放
    void downloads.refresh()
  } catch (e) {
    saveState.value = 'idle'
    ui.notify(`保存失败：${formatError(e)}`, 'error')
  }
}

/** 倍速切换（PlayerControls 弹出层） */
function onRate(value: number) {
  void setRate(value)
}

/* ---------------- 结束 / 片尾 ---------------- */

const canGoNext = computed(
  () => props.activeEpisode >= 0 && props.activeEpisode < props.episodes.length - 1,
)

watch(
  () => playerState.value.status,
  (status) => {
    if (status !== 'ended') return
    if (loop.value) {
      void replay()
    }
  },
)

/** 播放到片尾标记：自动下一集（没有下一集则停在结束态） */
watch(
  () => playerState.value.positionSec,
  (position) => {
    if (outroFired || status.value !== 'playing') return
    const outro = outroSec.value
    if (outro === null || position < outro) return
    outroFired = true
    if (loop.value) {
      void replay()
      return
    }
    if (canGoNext.value) {
      ui.notify('已跳过片尾，播放下一集', 'info')
      emit('next')
    }
  },
)

/* ---------------- 状态派生 ---------------- */

/** 有媒体且播放器已接管（idle/error 时不显示原生窗口） */
const mediaActive = computed(
  () => props.hasMedia && !['idle', 'error'].includes(playerState.value.status),
)

/** 原生视频窗口真正应可见（演示模式无原生窗口） */
const stageLive = computed(() => mediaActive.value && isTauri)

/** 视频洞已开启（有真实画面）：此时窗格被裁剪，交互浮层走 Teleport */
const teleportActive = computed(() => videoRect.value !== null)

/** Teleport 浮层坐标：与 usePlayer 上报的 CSS rect 完全一致（视口坐标） */
const overlayStyle = computed(() => {
  const rect = videoRect.value
  if (!rect) return undefined
  return {
    left: `${rect.x}px`,
    top: `${rect.y}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
  }
})

const status = computed(() => playerState.value.status)

/** HDR / 宽色域标识（来自 mpv video-params；SDR 片源为空，不显示徽标） */
const hdrLabel = computed(() => {
  const gamma = playerState.value.videoGamma
  const primaries = playerState.value.videoPrimaries
  if (gamma === 'pq') return 'HDR10'
  if (gamma === 'hlg') return 'HLG'
  if (primaries === 'bt.2020') return '宽色域'
  return ''
})

/** 播放失败：只在「进入 error 的瞬间」抛一次，避免 5Hz 状态刷新反复触发页面回退逻辑 */
watch(
  () => playerState.value.status,
  (next, prev) => {
    if (next === 'error' && prev !== 'error') {
      emit('error', playerState.value.error ?? '')
    }
  },
)

/** 解析 / 打开中的文案 */
const openingText = computed(() => {
  if (props.loadingText) return props.loadingText
  if (props.loading) return '正在解析直链…'
  return '正在打开播放器…'
})

const shownError = computed(() =>
  explainMediaError(playerState.value.error || props.errorText, 'play'),
)

/** 播放器/解析错误态（无可用媒体时占满窗格） */
const showErrorOverlay = computed(
  () =>
    status.value === 'error' ||
    (!props.hasMedia && Boolean(props.errorText) && !props.loading),
)

const showIdleOverlay = computed(
  () => !props.hasMedia && !props.loading && !props.errorText,
)

/** 演示模式：没有原生视频，选中集后展示占位说明 */
const showDemoOverlay = computed(() => !isTauri && props.hasMedia && !props.loading)

const showOpeningOverlay = computed(
  () =>
    !showIdleOverlay.value &&
    !showErrorOverlay.value &&
    (props.loading || status.value === 'opening'),
)

const showEndedOverlay = computed(
  () => !showErrorOverlay.value && status.value === 'ended',
)

/** 有媒体时解析新集：小胶囊提示，不遮挡正在播放的画面 */
const showParsingPill = computed(
  () => props.loading && props.hasMedia && Boolean(playerState.value.url),
)

const showBuffering = computed(
  () => status.value === 'buffering' && !showOpeningOverlay.value && !props.loading,
)

/* ---------------- 控制条 / 顶部信息条显隐（鼠标静止 3s 自动隐藏） ---------------- */

/** 鼠标静止多久后收起控制条（全屏下鼠标停在画面里不动同样生效） */
const HIDE_DELAY_MS = 3000
const showControls = ref(false)
/** 控制条弹层（选集/字幕/音轨/音量/片头片尾）是否打开：打开时保持常显 */
const panelOpen = ref(false)
let hideTimer: number | null = null

/** 顶部信息条 / 控制条组件引用：用于判断鼠标是否停在「标题 / 按钮 / 进度条」区域 */
const topBarRef = ref<InstanceType<typeof PlayerTopBar> | null>(null)
const controlsRef = ref<InstanceType<typeof PlayerControls> | null>(null)
/** 最后一次鼠标位置（视口坐标）：静止收起前用它复查是否还停在控制区上 */
let lastPointer: { x: number; y: number } | null = null
/** 鼠标是否停在顶部信息条 / 控制条区域内：停住时保持常显、不自动收起 */
const pointerOverUi = ref(false)

function elementRect(target: { $el?: unknown } | null | undefined): DOMRect | null {
  const el = target?.$el
  return el instanceof HTMLElement ? el.getBoundingClientRect() : null
}

/** 判断视口坐标是否落在顶部信息条 / 控制条（含进度条）的矩形内 */
function isPointerOverUi(x: number, y: number): boolean {
  for (const rect of [elementRect(topBarRef.value), elementRect(controlsRef.value)]) {
    if (rect && x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
      return true
    }
  }
  return false
}

/** 顶部信息条与控制条同一显隐节奏：鼠标活动时显示；
 *  鼠标停在控制条 / 标题条 / 进度条上时保持常显（不自动收起），移开后按静止计时收起 */
const controlsVisible = computed(
  () => props.hasMedia && (showControls.value || panelOpen.value || pointerOverUi.value),
)

/** 控制条弹层开关（弹层 teleport 到 body，鼠标会离开窗格） */
function onPanelToggle(open: boolean) {
  panelOpen.value = open
  if (open) {
    showControls.value = true
    clearHideTimer()
    return
  }
  scheduleHide()
}

function clearHideTimer() {
  if (hideTimer !== null) {
    window.clearTimeout(hideTimer)
    hideTimer = null
  }
}

/** 重新计时：HIDE_DELAY_MS 内没有新的鼠标活动就收起；
 *  收起前用最后一次指针位置复查：还停在控制条 / 标题条 / 进度条上则保持常显 */
function scheduleHide() {
  clearHideTimer()
  if (panelOpen.value) return
  hideTimer = window.setTimeout(() => {
    hideTimer = null
    if (panelOpen.value) return
    if (lastPointer && isPointerOverUi(lastPointer.x, lastPointer.y)) {
      pointerOverUi.value = true
      return
    }
    pointerOverUi.value = false
    showControls.value = false
  }, HIDE_DELAY_MS)
}

/** 鼠标移入 / 在窗格/浮层内移动：显示控制条、记录指针位置并重置淡出计时 */
function onActivity(event?: MouseEvent) {
  if (!props.hasMedia) return
  showControls.value = true
  if (event) {
    lastPointer = { x: event.clientX, y: event.clientY }
    pointerOverUi.value = isPointerOverUi(event.clientX, event.clientY)
  }
  scheduleHide()
}

/** 鼠标移出窗格：复位「停在控制区」状态，同样按静止计时收起（移回立即恢复） */
function onLeave() {
  pointerOverUi.value = false
  lastPointer = null
  scheduleHide()
}

// 洞关闭（错误/离开播放）时复位控制条状态，避免浮层再次出现时控制条常驻
watch(teleportActive, (active) => {
  if (!active) {
    showControls.value = false
    pointerOverUi.value = false
    lastPointer = null
    clearHideTimer()
    clearClickTimer()
  }
})

/** 控制条隐藏时隐藏鼠标指针（视频区域） */
const hideCursor = computed(() => stageLive.value && !controlsVisible.value)

/* ---------------- 原生窗口可见性 / 点击 ---------------- */

watch(
  () => mediaActive.value,
  (active) => {
    void setVisible(active)
    if (active) syncRect(stageRef.value)
  },
)

watch(fullscreen, () => {
  if (paneDisposed) return
  // 进出全屏都会改变窗格几何：先同步一次，再在窗口尺寸稳定后补几次
  syncRect(stageRef.value)
  scheduleFullscreenSync()
})

/** 点击视频区域：播放/暂停（idle / error / ended / opening 下不响应） */
function onSurfaceClick() {
  if (!props.hasMedia || props.loading) return
  if (['idle', 'error', 'ended', 'opening'].includes(status.value)) return
  void togglePause()
}

/* ---------------- Teleport 浮层交互（区分单击 toggle / 双击全屏） ---------------- */

let clickTimer: number | null = null

function clearClickTimer() {
  if (clickTimer !== null) {
    window.clearTimeout(clickTimer)
    clickTimer = null
  }
}

/** 单击：延迟 220ms 执行，避免双击时连触两次播放/暂停 */
function onOverlayClick() {
  if (clickTimer !== null) return
  clickTimer = window.setTimeout(() => {
    clickTimer = null
    onSurfaceClick()
  }, 220)
}

/** 双击：窗格全屏 */
function onOverlayDblClick() {
  clearClickTimer()
  void toggleFullscreen()
}

/* ---------------- 控制条事件 ---------------- */

function onToggle() {
  void togglePause()
}

function onSeek(sec: number) {
  void seek(sec)
}

function onVolume(value: number) {
  void setVolume(value)
}

function onMute() {
  void setMute(!playerState.value.muted)
}

function onPrev() {
  emit('prev')
}

function onNext() {
  emit('next')
}

function onSelectEpisode(index: number) {
  emit('select-episode', index)
}

/* ---------------- 对外 API（详情页通过 ref 调用） ---------------- */

defineExpose({
  open,
  close,
  /** 播放器状态 ref（expose proxy 会自动解包）：页面可直接响应式读取速度 / 缓存等字段 */
  state: playerState,
  /** 供页面在离开前写回历史进度 */
  getState: (): PlayerState => playerState.value,
})
</script>

<template>
  <div
    class="player-pane"
    :class="{
      'is-fullscreen': fullscreen,
      'is-mini': mini,
      'is-window': windowMode,
      'is-live': stageLive,
      'hide-cursor': hideCursor,
    }"
    @mouseenter="onActivity"
    @mousemove="onActivity"
    @mouseleave="onLeave"
  >
    <!-- 独立播放器窗口：右上角 HDR 标识 + 关闭按钮（悬浮层，鼠标移开随控制条一起淡出） -->
    <div v-if="windowMode" class="window-chrome" :class="{ 'is-hidden': !controlsVisible }">
      <span v-if="hdrLabel" class="hdr-badge">{{ hdrLabel }}</span>
      <button
        class="window-close"
        type="button"
        title="关闭播放器窗口"
        aria-label="关闭播放器窗口"
        @click.stop="emit('close-window')"
      >
        <SolarIcon name="close" :size="18" />
      </button>
    </div>

    <div
      ref="stageRef"
      class="player-stage"
      :class="{ 'is-live': stageLive, 'is-fullscreen': fullscreen }"
    >
      <!-- 视频「洞」：播放态完全透明，露出下层原生窗口（洞开启时整个窗格被 clip-path 裁掉） -->
      <div class="video-hole" @click="onSurfaceClick" />

      <!-- 以下浮层只在「视频洞未开启」时渲染（洞开启后由下方 Teleport 浮层接管） -->
      <template v-if="!teleportActive">
        <!-- ① idle：封面模糊背景 + 中央提示（此时无原生视频，可以有整块底色） -->
        <div v-if="showIdleOverlay" class="overlay idle-overlay">
          <img v-if="cover" class="idle-cover" :src="cover" alt="" aria-hidden="true" />
          <div class="idle-scrim" aria-hidden="true" />
          <div class="overlay-body">
            <SolarIcon class="overlay-icon" name="videocamera" :size="40" />
            <p class="overlay-title">选择剧集开始播放</p>
            <p class="overlay-desc">点下方任意一集，解析成功后直接在此窗格播放</p>
          </div>
        </div>

        <!-- ② opening：解析 / 打开播放器（转圈 + 状态文案） -->
        <div v-else-if="showOpeningOverlay" class="overlay opening-overlay">
          <img
            v-if="cover && !stageLive"
            class="idle-cover"
            :src="cover"
            alt=""
            aria-hidden="true"
          />
          <div class="idle-scrim" aria-hidden="true" />
          <div class="overlay-body">
            <SolarIcon class="overlay-icon spin" name="refresh-circle" :size="34" />
            <p class="overlay-title">{{ openingText }}</p>
            <p v-if="title" class="overlay-desc ellipsis-1">{{ title }}</p>
          </div>
        </div>

        <!-- ③ 演示模式：无原生视频，仅占位说明（不伪造播放画面） -->
        <div v-else-if="showDemoOverlay" class="overlay demo-overlay">
          <img v-if="cover" class="idle-cover" :src="cover" alt="" aria-hidden="true" />
          <div class="idle-scrim" aria-hidden="true" />
          <div class="overlay-body">
            <SolarIcon class="overlay-icon" name="videocamera" :size="36" />
            <p class="overlay-title">演示模式不渲染视频</p>
            <p class="overlay-desc">本地播放器（libmpv）仅在桌面端可用；直链解析流程已就绪</p>
          </div>
        </div>

        <!-- ④ error：错误文案 + 重试 -->
        <div v-else-if="showErrorOverlay" class="overlay error-overlay">
          <div class="overlay-body">
            <SolarIcon class="overlay-icon error-icon" name="danger-triangle" :size="34" />
            <p class="overlay-title">无法播放</p>
            <p class="overlay-error" :title="shownError">{{ shownError || '播放器未返回可用地址' }}</p>
            <el-button size="small" type="primary" @click.stop="emit('retry')">
              <template #icon><SolarIcon name="refresh" /></template>
              重试
            </el-button>
          </div>
        </div>

        <!-- ⑤ ended（洞未开时是兜底；正常由 Teleport 渲染） -->
        <div v-else-if="showEndedOverlay" class="overlay ended-overlay">
          <div class="overlay-body">
            <SolarIcon class="overlay-icon" name="pause" :size="34" />
            <p class="overlay-title">播放结束</p>
            <el-button size="small" type="primary" @click.stop="emit('retry')">
              <template #icon><SolarIcon name="play" /></template>
              重播
            </el-button>
          </div>
        </div>

        <!-- 缓冲中：小转圈（不遮挡整块画面） -->
        <div v-if="showBuffering" class="buffering-badge" aria-label="缓冲中">
          <SolarIcon class="spin" name="refresh-circle" :size="22" />
        </div>

        <!-- 有媒体时解析新集：顶部胶囊提示，不盖住当前画面 -->
        <div v-if="showParsingPill" class="parsing-pill" role="status">
          <SolarIcon class="spin" name="refresh-circle" :size="12" />
          <span class="ellipsis-1">{{ openingText }}</span>
        </div>

        <!-- 顶部信息条 + 控制条（未开洞时仍在窗格内） -->
        <PlayerTopBar
          ref="topBarRef"
          v-if="hasMedia"
          :title="seriesName"
          :episode-label="episodeLabel"
          :episode-title="title"
          :width="playerState.width"
          :height="playerState.height"
          :speed-bps="playerState.bufferSpeedBps"
          :hwdec="playerState.hwdecCurrent"
          :video-ready="playerState.videoReady"
          :container-fps="playerState.containerFps"
          :estimated-fps="playerState.estimatedFps"
          :audio-label="activeAudioLabel"
          :visible="controlsVisible"
        />

        <PlayerControls
          ref="controlsRef"
          v-if="hasMedia"
          :state="playerState"
          :visible="controlsVisible"
          :fullscreen="fullscreen"
          :mini="mini"
          :allow-mini="!windowMode"
          :allow-pop-out="!windowMode"
          :disabled="status === 'error'"
          :loop="loop"
          :danmaku="danmaku"
          :episodes="episodes"
          :active-episode="activeEpisode"
          :subtitle-tracks="subtitleTracks"
          :audio-tracks="audioTracks"
          :active-subtitle="activeSubtitle"
          :active-audio="activeAudio"
          :enhance="enhance"
          :plugin-items="pluginItems"
          :intro-sec="introSec"
          :outro-sec="outroSec"
          :can-save="canSaveEpisode"
          :save-state="saveState"
          :auto-save="autoSave"
          @toggle="onToggle"
          @seek="onSeek"
          @volume="onVolume"
          @mute="onMute"
          @prev="onPrev"
          @next="onNext"
          @replay="onReplay"
          @select-episode="onSelectEpisode"
          @toggle-loop="toggleLoop"
          @toggle-danmaku="toggleDanmaku"
          @select-subtitle="onSelectSubtitle"
          @load-subtitle="onLoadSubtitle"
          @search-online-subtitle="onSearchOnlineSubtitle"
          @select-audio="onSelectAudio"
          @enhance="onEnhance"
          @toggle-plugin="onTogglePlugin"
          @select-plugin-preset="onSelectPluginPreset"
          @rate="onRate"
          @toggle-mini="toggleMini"
          @fullscreen="toggleFullscreen"
          @pop-out="emit('pop-out')"
          @set-intro="setIntro"
          @set-outro="setOutro"
          @clear-intro="clearIntro"
          @clear-outro="clearOutro"
          @save-episode="onSaveEpisode"
          @toggle-auto-save="emit('toggle-auto-save')"
          @panel-toggle="onPanelToggle"
        />
      </template>
    </div>
  </div>

  <!-- 视频洞开启时：交互层 + 控制条 + 标题 + 缓冲/结束浮层（fixed，位于洞的上层）。
       根节点不绘制背景，只有控制条渐变 / 标题 / 状态胶囊自身有背景。 -->
  <Teleport to="body">
    <div
      v-if="teleportActive && videoRect"
      class="player-overlay"
      :class="{ 'hide-cursor': hideCursor }"
      :style="overlayStyle"
      @mouseenter="onActivity"
      @mousemove="onActivity"
      @mouseleave="onLeave"
    >
      <!-- 透明点击层：单击播放/暂停、双击全屏；整个洞内可交互 -->
      <div
        class="overlay-hit"
        role="button"
        tabindex="-1"
        aria-label="播放器画面（单击播放/暂停，双击全屏）"
        @click="onOverlayClick"
        @dblclick="onOverlayDblClick"
      />

      <!-- 缓冲中 -->
      <div v-if="showBuffering" class="buffering-badge" aria-label="缓冲中">
        <SolarIcon class="spin" name="refresh-circle" :size="22" />
      </div>

      <!-- 解析新集胶囊 -->
      <div v-if="showParsingPill" class="parsing-pill" role="status">
        <SolarIcon class="spin" name="refresh-circle" :size="12" />
        <span class="ellipsis-1">{{ openingText }}</span>
      </div>

      <!-- 顶部信息条（剧集名/集数、分辨率、当前时间） -->
      <PlayerTopBar
        ref="topBarRef"
        v-if="hasMedia"
        :title="seriesName"
        :episode-label="episodeLabel"
        :episode-title="title"
        :width="playerState.width"
        :height="playerState.height"
        :speed-bps="playerState.bufferSpeedBps"
        :hwdec="playerState.hwdecCurrent"
        :video-ready="playerState.videoReady"
        :container-fps="playerState.containerFps"
        :estimated-fps="playerState.estimatedFps"
        :audio-label="activeAudioLabel"
        :visible="controlsVisible"
      />

      <!-- 播放结束（覆盖在最后一帧上；按钮可交互，其余区域不拦截点击层） -->
      <div v-if="showEndedOverlay" class="overlay ended-overlay overlay-pass">
        <div class="overlay-body">
          <SolarIcon class="overlay-icon" name="pause" :size="34" />
          <p class="overlay-title">播放结束</p>
          <el-button size="small" type="primary" @click.stop="emit('retry')">
            <template #icon><SolarIcon name="play" /></template>
            重播
          </el-button>
        </div>
      </div>

      <!-- 控制条 -->
      <PlayerControls
        ref="controlsRef"
        :state="playerState"
        :visible="controlsVisible"
        :fullscreen="fullscreen"
        :mini="mini"
          :allow-mini="!windowMode"
          :allow-pop-out="!windowMode"
        :loop="loop"
        :danmaku="danmaku"
        :episodes="episodes"
        :active-episode="activeEpisode"
        :subtitle-tracks="subtitleTracks"
        :audio-tracks="audioTracks"
        :active-subtitle="activeSubtitle"
        :active-audio="activeAudio"
        :enhance="enhance"
        :plugin-items="pluginItems"
        :intro-sec="introSec"
        :outro-sec="outroSec"
        :can-save="canSaveEpisode"
        :save-state="saveState"
        :auto-save="autoSave"
        @toggle="onToggle"
        @seek="onSeek"
        @volume="onVolume"
        @mute="onMute"
        @prev="onPrev"
        @next="onNext"
        @replay="onReplay"
        @select-episode="onSelectEpisode"
        @toggle-loop="toggleLoop"
        @toggle-danmaku="toggleDanmaku"
        @select-subtitle="onSelectSubtitle"
        @load-subtitle="onLoadSubtitle"
        @search-online-subtitle="onSearchOnlineSubtitle"
        @select-audio="onSelectAudio"
        @enhance="onEnhance"
        @toggle-plugin="onTogglePlugin"
        @select-plugin-preset="onSelectPluginPreset"
        @rate="onRate"
        @toggle-mini="toggleMini"
        @fullscreen="toggleFullscreen"
        @pop-out="emit('pop-out')"
        @set-intro="setIntro"
        @set-outro="setOutro"
        @clear-intro="clearIntro"
        @clear-outro="clearOutro"
        @save-episode="onSaveEpisode"
        @toggle-auto-save="emit('toggle-auto-save')"
        @panel-toggle="onPanelToggle"
      />
    </div>
  </Teleport>

  <!-- 在线字幕搜索弹窗（teleport 到 body，与视频洞/全屏状态无关） -->
  <SubtitleSearchDialog
    v-model="subtitleDialogOpen"
    :default-query="onlineSubtitleQuery"
    @loaded="onOnlineSubtitleLoaded"
  />
</template>

<style scoped>
.player-pane {
  position: relative;
  width: 100%;
  min-width: 0;
}

/* 16:9 窗格：普通状态带圆角；播放态切成方角（原生窗口不能圆角裁剪） */
.player-stage {
  position: relative;
  width: 100%;
  aspect-ratio: 16 / 9;
  overflow: hidden;
  background: linear-gradient(
    160deg,
    color-mix(in srgb, var(--el-color-black) 84%, var(--surface)),
    color-mix(in srgb, var(--el-color-black) 94%, var(--surface))
  );
  border-radius: var(--radius-lg);
  outline: 1px solid var(--border);
  outline-offset: -1px;
  box-shadow: var(--shadow-1);
}

/* 播放态：整块透明（原生窗口在下层），方角 + 细描边 */
.player-stage.is-live {
  background: transparent;
  border-radius: 0;
  outline-color: color-mix(in srgb, var(--border) 70%, transparent);
}

/* 视频洞：不画任何背景，只接收点击 */
.video-hole {
  position: absolute;
  inset: 0;
  z-index: 0;
  cursor: pointer;
}

.player-pane.hide-cursor .video-hole {
  cursor: none;
}

/* ---------------- 全屏（窗格全屏） ---------------- */

.player-pane.is-fullscreen {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  background: transparent;
}

/* 全屏：窗格直接铺满当前显示器视口（宽高比交给 mpv 内部 letterbox）。
   这样「视频洞」= 整个视口，mpv 的 video-margin-ratio 也覆盖整个窗口，
   超宽/超高的片源不会出现外层 16:9 与内层黑边叠加的双层黑边。 */
.player-stage.is-fullscreen {
  width: 100vw;
  height: 100vh;
  aspect-ratio: auto;
  border-radius: 0;
  outline: none;
  box-shadow: none;
}

/* ---------------- 小窗：只保留播放器窗格（铺满窗口，其它界面全部盖住） ---------------- */

.player-pane.is-mini {
  position: fixed;
  inset: 0;
  z-index: 120;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  background: #000;
}

.player-pane.is-mini .player-stage {
  width: min(100vw, calc(100vh * 16 / 9));
  height: auto;
  border-radius: 0;
  outline: none;
}

/* ---------------- Teleport 浮层（洞上层；根节点不画背景） ---------------- */

.player-overlay {
  position: fixed;
  z-index: 32;
  overflow: hidden;
  /* 只让点击层/控件自身接收事件，根不遮挡 */
  pointer-events: none;
}

.player-overlay.hide-cursor {
  cursor: none;
}

/* 透明点击层：铺满视频洞，负责 hover 计时、单击 toggle、双击全屏 */
.overlay-hit {
  position: absolute;
  inset: 0;
  z-index: 0;
  background: transparent;
  pointer-events: auto;
  cursor: pointer;
}

.player-overlay.hide-cursor .overlay-hit {
  cursor: none;
}

/* ---------------- 状态浮层（小/整块浮层都在 webview 层，可带背景） ---------------- */

.overlay {
  position: absolute;
  inset: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  padding: var(--space-4);
  text-align: center;
}

/* 结束态浮层：自身不拦截点击（只让内部按钮可点） */
.overlay-pass {
  pointer-events: none;
}

.overlay-pass :deep(.el-button) {
  pointer-events: auto;
}

.idle-overlay,
.demo-overlay,
.opening-overlay {
  background: color-mix(in srgb, var(--el-color-black) 62%, transparent);
}

.error-overlay {
  background: color-mix(in srgb, var(--el-color-black) 78%, transparent);
}

.ended-overlay {
  background: color-mix(in srgb, var(--el-color-black) 66%, transparent);
}

/* 封面模糊底：只在没有原生视频的状态下出现 */
.idle-cover {
  position: absolute;
  inset: -24px;
  width: calc(100% + 48px);
  height: calc(100% + 48px);
  object-fit: cover;
  filter: blur(26px) brightness(0.58) saturate(0.95);
  transform: scale(1.04);
  pointer-events: none;
}

.idle-scrim {
  position: absolute;
  inset: 0;
  background: radial-gradient(
    circle at 50% 45%,
    color-mix(in srgb, var(--el-color-black) 26%, transparent),
    color-mix(in srgb, var(--el-color-black) 68%, transparent) 78%
  );
}

.overlay-body {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  max-width: 86%;
}

/* 深色视频底上的操作按钮：白色玻璃质感，避免主题主色对比不足 */
.overlay-body :deep(.el-button--primary) {
  --el-button-bg-color: rgba(255, 255, 255, 0.16);
  --el-button-border-color: rgba(255, 255, 255, 0.42);
  --el-button-text-color: rgba(255, 255, 255, 0.95);
  --el-button-hover-bg-color: rgba(255, 255, 255, 0.26);
  --el-button-hover-border-color: rgba(255, 255, 255, 0.6);
  --el-button-hover-text-color: #fff;
  --el-button-active-bg-color: rgba(255, 255, 255, 0.3);
  --el-button-active-border-color: rgba(255, 255, 255, 0.68);
}

.overlay-icon {
  color: rgba(255, 255, 255, 0.82);
}

.error-icon {
  color: var(--danger);
}

.overlay-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.94);
}

.overlay-desc {
  margin: 0;
  max-width: 100%;
  font-size: 12.5px;
  line-height: 1.6;
  color: rgba(255, 255, 255, 0.66);
}

.overlay-error {
  margin: 0;
  max-width: 100%;
  max-height: 74px;
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: 11.5px;
  line-height: 1.5;
  color: rgba(255, 255, 255, 0.76);
  overflow-wrap: anywhere;
}

/* 缓冲小转圈 */
.buffering-badge {
  position: absolute;
  top: 50%;
  left: 50%;
  z-index: 2;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 48px;
  height: 48px;
  border-radius: var(--radius-pill);
  background: rgba(0, 0, 0, 0.5);
  color: #fff;
  transform: translate(-50%, -50%);
  pointer-events: none;
}

/* 解析胶囊：顶部居中（不遮挡画面主体） */
.parsing-pill {
  position: absolute;
  top: var(--space-3);
  left: 50%;
  z-index: 2;
  display: flex;
  gap: var(--space-1);
  align-items: center;
  max-width: 72%;
  padding: 4px 10px;
  border-radius: var(--radius-pill);
  background: rgba(0, 0, 0, 0.62);
  color: rgba(255, 255, 255, 0.9);
  font-size: 12px;
  transform: translateX(-50%);
  pointer-events: none;
}

/* 左上角标题已由 PlayerTopBar 接管（跟随控制条显隐） */

.spin {
  animation: player-spin 1s linear infinite;
}

@keyframes player-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .spin {
    animation: none;
  }
}
/* ---------------- 独立播放器窗口模式（player-window） ---------------- */

.player-pane.is-window {
  position: fixed;
  inset: 0;
  width: 100%;
  height: 100%;
  background: transparent;
}

.player-pane.is-window .player-stage {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  aspect-ratio: auto;
  border-radius: 0;
  outline: none;
  box-shadow: none;
  background: linear-gradient(160deg, #000 0%, #0b0d12 100%);
}

.player-pane.is-window .player-stage.is-live {
  background: transparent;
}

/* 窗口右上角的 chrome（关闭按钮 + HDR 标识），跟随控制条显隐 */
.window-chrome {
  position: fixed;
  top: 12px;
  right: 14px;
  z-index: 2147483000;
  display: flex;
  align-items: center;
  gap: 8px;
  transition: opacity 0.25s ease;
}

.window-chrome.is-hidden {
  opacity: 0;
  pointer-events: none;
}

.window-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: none;
  border-radius: 50%;
  color: #fff;
  background: rgba(0, 0, 0, 0.45);
  cursor: pointer;
  transition: background 0.15s ease;
}

.window-close:hover {
  background: rgba(220, 38, 38, 0.85);
}

.hdr-badge {
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.4px;
  color: #0b0d12;
  background: linear-gradient(135deg, #f5d76e, #e8b84b);
  box-shadow: 0 1px 6px rgba(0, 0, 0, 0.35);
}
</style>

<style>
/* 播放器「真·全屏」时隐藏应用外壳（标题栏 / 侧栏），只保留视频与控制条浮层。
   标记写在 <html data-player-fullscreen> 上（不是 class）：ui store 的 applyTheme 会整体重写
   <html class="...">，用 class 会在切主题时丢失。这里不改 layout 组件，避免跨模块耦合。 */
html[data-player-fullscreen] .titlebar,
html[data-player-fullscreen] .sidebar {
  display: none !important;
}

html[data-player-fullscreen] .app-shell {
  background: #000;
}

/* 全屏视口不需要内容区圆角与滚动条 */
html[data-player-fullscreen] .app-content {
  border-radius: 0 !important;
}

html[data-player-fullscreen] .page-scroll .el-scrollbar__bar {
  display: none !important;
}
</style>
