// ============================================================
// 播放器组合式函数（libmpv 原生子窗口）
// - 订阅后端 `player-state` 事件（约 5Hz），维护前端 PlayerState
// - 封装 player_* 命令；非 Tauri（浏览器预览）一律 no-op，绝不把异常抛到页面
// - syncRect(el)：把播放器窗格的可视矩形按物理像素上报给后端原生窗口，
//   用 requestAnimationFrame 节流，并在 ResizeObserver / window resize /
//   内容滚动容器 `.app-content .el-scrollbar__wrap` 的 scroll(capture) 上重算。
// - 组件卸载时自动解绑事件、断开 rect 监听并隐藏原生窗口。
// 后端契约见 src/types/index.ts 的 PlayerState / PlayerStatus。
// ============================================================
import { onMounted, onUnmounted, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import * as api from '@/api'
import type { PlayerState, PlayerStatus, PlayerTrack } from '@/types'

/** 合法状态集合（后端新增状态时前端保守回退到 idle） */
const PLAYER_STATUSES: readonly PlayerStatus[] = [
  'idle',
  'opening',
  'playing',
  'paused',
  'buffering',
  'ended',
  'error',
]

/** 只有这些状态才可能有真实视频画面（用于开启 .app-shell 的视频洞） */
const HOLE_STATUSES: readonly PlayerStatus[] = ['playing', 'paused', 'buffering', 'ended']

/** 数值容错：非法值回退到缺省 */
function num(value: unknown, fallback = 0): number {
  const n = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(n) ? n : fallback
}

/** 轨道数组容错：字段缺失/类型不符时丢条目而不是抛错 */
function normalizeTracks(value: unknown): PlayerTrack[] {
  if (!Array.isArray(value)) return []
  const tracks: PlayerTrack[] = []
  for (const item of value) {
    if (!item || typeof item !== 'object') continue
    const track = item as Partial<PlayerTrack>
    if (track.id === undefined || track.id === null) continue
    tracks.push({
      id: String(track.id),
      label: typeof track.label === 'string' && track.label ? track.label : `轨道 ${track.id}`,
      lang: typeof track.lang === 'string' && track.lang ? track.lang : undefined,
      codec: typeof track.codec === 'string' && track.codec ? track.codec : undefined,
      external: Boolean(track.external),
    })
  }
  return tracks
}

/** 事件载荷容错：字段缺失 / 类型不符时补缺省值（事件来自原生侧，不能假设完整） */
function normalizeState(raw: unknown): PlayerState {
  const base = api.playerIdleState()
  if (!raw || typeof raw !== 'object') return base
  const payload = raw as Partial<PlayerState>
  const status =
    typeof payload.status === 'string' && PLAYER_STATUSES.includes(payload.status)
      ? payload.status
      : 'idle'
  return {
    status,
    url: typeof payload.url === 'string' ? payload.url : '',
    positionSec: num(payload.positionSec),
    durationSec: num(payload.durationSec),
    paused: Boolean(payload.paused),
    buffering: Boolean(payload.buffering),
    volume: num(payload.volume, base.volume),
    muted: Boolean(payload.muted),
    rate: num(payload.rate, 1) || 1,
    width: num(payload.width),
    height: num(payload.height),
    cacheSec: num(payload.cacheSec),
    bufferSpeedBps: num(payload.bufferSpeedBps),
    videoReady: Boolean(payload.videoReady),
    containerFps: num(payload.containerFps),
    estimatedFps: num(payload.estimatedFps),
    hwdecCurrent: typeof payload.hwdecCurrent === 'string' ? payload.hwdecCurrent : '',
    displayFps: num(payload.displayFps),
    frameDrops: num(payload.frameDrops),
    decoderFrameDrops: num(payload.decoderFrameDrops),
    videoPrimaries: typeof payload.videoPrimaries === 'string' ? payload.videoPrimaries : '',
    videoGamma: typeof payload.videoGamma === 'string' ? payload.videoGamma : '',
    videoSigPeak: num(payload.videoSigPeak),
    audioTracks: normalizeTracks(payload.audioTracks),
    subtitleTracks: normalizeTracks(payload.subtitleTracks),
    activeAudioId:
      typeof payload.activeAudioId === 'string' && payload.activeAudioId
        ? payload.activeAudioId
        : 'no',
    activeSubtitleId:
      typeof payload.activeSubtitleId === 'string' && payload.activeSubtitleId
        ? payload.activeSubtitleId
        : 'no',
    error: typeof payload.error === 'string' ? payload.error : '',
  }
}

/** 播放器区域矩形（物理像素） */
interface PixelRect {
  x: number
  y: number
  width: number
  height: number
}

/** 播放器区域矩形（CSS 像素 / 视口坐标；用于 clip-path 挖洞与 teleport 浮层定位） */
interface CssRect {
  x: number
  y: number
  width: number
  height: number
}

/** 从播放器窗格向上找它所属页面的滚动容器（缓存页面可能同时留在 DOM 里） */
function pageScrollWrap(el: HTMLElement): HTMLElement | null {
  return el.closest<HTMLElement>('.page-scroll')?.querySelector<HTMLElement>('.el-scrollbar__wrap') ?? null
}

export function usePlayer() {
  /** 播放器状态（初始 idle；Tauri 下由 player-state 事件驱动） */
  const state = ref<PlayerState>(api.playerIdleState())

  /** 当前「视频洞」矩形（CSS px / 视口坐标）；null = 洞关闭（无原生画面） */
  const videoRect = ref<CssRect | null>(null)

  let unlisten: UnlistenFn | null = null
  let disposed = false

  /* ---------------- 事件订阅 ---------------- */

  onMounted(() => {
    if (!api.isTauri) return
    void listen<PlayerState>('player-state', (event) => {
      if (disposed) return
      state.value = normalizeState(event.payload)
    })
      .then((fn) => {
        if (disposed) {
          fn()
          return
        }
        unlisten = fn
      })
      .catch(() => {
        // 事件总线不可用：保留本地乐观状态，不阻塞页面
      })
  })

  onUnmounted(() => {
    disposed = true
    if (unlisten) {
      unlisten()
      unlisten = null
    }
    stopRectSync()
  })

  /* ---------------- 命令封装（永不抛错） ---------------- */

  /** 乐观更新本地状态（事件到达后会被后端状态覆盖） */
  function patch(patchState: Partial<PlayerState>) {
    state.value = { ...state.value, ...patchState }
  }

  /** 演示模式 / 未就绪时的失败兜底：只降级为 error 态，不抛出 */
  function fail(e: unknown) {
    patch({ status: 'error', error: api.formatError(e), buffering: false })
  }

  /** 打开直链；后端未就绪时把错误落到 error 态（播放器窗格展示重试） */
  async function open(
    url: string,
    headers?: Record<string, string>,
    startSec?: number,
  ): Promise<void> {
    // 浏览器预览没有原生窗口：保留 idle，由 PlayerPane 展示占位说明
    // （Tauri mock 演示模式下 demoMode 同样为 true，但播放走真实 mpv，不能在这里拦）
    if (!api.isTauri) {
      patch({ status: 'idle', url, positionSec: startSec ?? 0, error: '', paused: false })
      return
    }
    // 先进入 opening：解析成功 → 等后端首个 player-state 事件接管
    patch({
      status: 'opening',
      url,
      positionSec: startSec ?? 0,
      durationSec: 0,
      paused: false,
      buffering: true,
      cacheSec: 0,
      bufferSpeedBps: 0,
      videoReady: false,
      audioTracks: [],
      subtitleTracks: [],
      activeAudioId: 'no',
      activeSubtitleId: 'no',
      error: '',
    })
    try {
      await api.playerOpen(url, headers, startSec)
    } catch (e) {
      fail(e)
    }
  }

  /** 关闭播放器并复位状态 */
  async function close(): Promise<void> {
    try {
      if (api.isTauri) await api.playerClose()
    } catch {
      // 关闭失败不影响页面离开
    } finally {
      state.value = api.playerIdleState()
    }
  }

  async function play(): Promise<void> {
    if (!api.isTauri) return
    try {
      await api.playerPlay()
      patch({ paused: false })
    } catch {
      // 播放命令失败由状态事件体现，避免弹错
    }
  }

  async function pause(): Promise<void> {
    if (!api.isTauri) return
    try {
      await api.playerPause()
      patch({ paused: true })
    } catch {
      // 同上
    }
  }

  /** 切换播放/暂停：以后端返回值为准，失败时按本地 paused 取反 */
  async function togglePause(): Promise<void> {
    if (!api.isTauri) return
    try {
      const paused = await api.playerTogglePause()
      patch({ paused, status: paused ? 'paused' : 'playing' })
    } catch {
      const paused = !state.value.paused
      patch({ paused, status: paused ? 'paused' : 'playing' })
    }
  }

  async function seek(sec: number): Promise<void> {
    if (!api.isTauri) return
    const target = Math.max(0, sec)
    patch({ positionSec: target })
    try {
      await api.playerSeek(target)
    } catch {
      // 跳转失败不弹错；下一次状态事件会纠正进度
    }
  }

  /** 音量 0-100 */
  async function setVolume(volume: number): Promise<void> {
    if (!api.isTauri) return
    const value = Math.min(100, Math.max(0, Math.round(volume)))
    patch({ volume: value, muted: value > 0 ? false : state.value.muted })
    try {
      await api.playerSetVolume(value)
    } catch {
      // 忽略
    }
  }

  async function setMute(muted: boolean): Promise<void> {
    if (!api.isTauri) return
    patch({ muted })
    try {
      await api.playerSetMute(muted)
    } catch {
      // 忽略
    }
  }

  async function setRate(rate: number): Promise<void> {
    if (!api.isTauri) return
    const value = Math.min(4, Math.max(0.25, rate))
    patch({ rate: value })
    try {
      await api.playerSetRate(value)
    } catch {
      // 忽略
    }
  }

  async function setVisible(visible: boolean): Promise<void> {
    wantsVisible = visible
    if (!api.isTauri) return
    // 先同步上报一次 rect / 洞，避免原生窗口或裁剪洞短暂停在旧位置
    pushRect()
    try {
      await api.playerSetVisible(visible)
    } catch {
      // 原生窗口未就绪时忽略
    }
  }

  /* ---------------- 矩形同步（物理像素 + rAF 节流） ---------------- */

  /** 需要上报的 DOM 元素（播放器窗格） */
  let targetEl: HTMLElement | null = null
  let resizeObserver: ResizeObserver | null = null
  /** 挂过 scroll 监听的容器（元素替换时需解绑） */
  let scrollEl: HTMLElement | null = null
  let rafId: number | null = null
  /** 上一次上报的矩形：值不变时不重复 invoke */
  let lastRect: PixelRect | null = null
  /** 上一次写入的视频洞 CSS 变量（key 化，避免逐帧写 DOM） */
  let lastHoleKey = ''
  /** 原生窗口期望可见（由 setVisible 控制） */
  let wantsVisible = false
  /** 上一次实际下发的可见性（与 wantsVisible/矩形可见性合成后） */
  let lastVisible = false

  /** 计算播放器窗格的可视矩形（CSS px / 视口坐标，裁剪到视口 + 内容滚动容器） */
  function computeCssRect(el: HTMLElement): CssRect | null {
    const box = el.getBoundingClientRect()
    // 只允许出现在内容滚动区内：避免浮层 fixed 定位时向上盖住 TitleBar / page-header。
    // 滚动容器的视口矩形 = 内容区（在标题栏下方、侧栏右侧）的可见范围。
    // 必须取播放器所在的 PageScroll，不能取 document 里第一个缓存页面的滚动区。
    const host = el.closest<HTMLElement>('.page-scroll')?.getBoundingClientRect()
    const left = Math.max(0, host?.left ?? 0, box.left)
    const top = Math.max(0, host?.top ?? 0, box.top)
    const right = Math.min(window.innerWidth, host?.right ?? window.innerWidth, box.right)
    const bottom = Math.min(
      window.innerHeight,
      host?.bottom ?? window.innerHeight,
      box.bottom,
    )
    const width = right - left
    const height = bottom - top
    if (width < 2 || height < 2) return null
    return { x: left, y: top, width, height }
  }

  /** CSS px 矩形 → 物理像素矩形（player_set_rect 用） */
  function toPixelRect(rect: CssRect): PixelRect {
    const dpr = window.devicePixelRatio || 1
    return {
      x: Math.round(rect.x * dpr),
      y: Math.round(rect.y * dpr),
      width: Math.round(rect.width * dpr),
      height: Math.round(rect.height * dpr),
    }
  }

  function sameRect(a: CssRect | null, b: CssRect | null): boolean {
    if (!a || !b) return a === b
    return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
  }

  function flushRect() {
    rafId = null
    pushRect()
  }

  /** 写 / 清 documentElement 上的视频洞变量（CSS px；值不变不重复写） */
  function applyHole(rect: CssRect | null): void {
    if (!api.isTauri) return
    const style = document.documentElement.style
    if (!rect) {
      if (lastHoleKey === 'closed') return
      lastHoleKey = 'closed'
      style.setProperty('--vbox-hole-w', '0px')
      style.setProperty('--vbox-hole-h', '0px')
      return
    }
    const px = (value: number) => `${Math.round(value * 100) / 100}px`
    const x = px(rect.x)
    const y = px(rect.y)
    const width = px(rect.width)
    const height = px(rect.height)
    const key = `${x},${y},${width},${height}`
    if (key === lastHoleKey) return
    lastHoleKey = key
    style.setProperty('--vbox-hole-x', x)
    style.setProperty('--vbox-hole-y', y)
    style.setProperty('--vbox-hole-w', width)
    style.setProperty('--vbox-hole-h', height)
  }

  /** 同步 videoRect（teleport 浮层定位用；避免逐帧触发无意义的重渲染） */
  function setVideoRect(next: CssRect | null): void {
    if (sameRect(videoRect.value, next)) return
    videoRect.value = next
  }

  /**
   * 计算并下发：物理 rect（原生窗口）+ 可见性 + CSS rect（视频洞 / teleport 浮层）。
   * 三者必须同一 rAF 节拍内更新，否则视频窗口与裁剪洞会错位。
   */
  function pushRect() {
    const el = targetEl
    if (!api.isTauri) return
    if (!el) {
      applyHole(null)
      setVideoRect(null)
      return
    }
    const cssRect = computeCssRect(el)
    // 洞开启条件：真实视频帧已输出 + 播放状态 + 期望可见 + 有效矩形。
    // opening / 首次缓冲阶段 mpv 还没有画面，若此时开洞会露出透明窗口（看到桌面），
    // 所以必须等 videoReady（后端 VIDEO_RECONFIG 事件）后再挖洞。
    const holeOn =
      Boolean(cssRect) &&
      wantsVisible &&
      state.value.videoReady &&
      HOLE_STATUSES.includes(state.value.status)
    applyHole(holeOn ? cssRect : null)
    setVideoRect(holeOn ? cssRect : null)

    const rect = cssRect ? toPixelRect(cssRect) : null
    // 滚出视口 / 尺寸为 0 时隐藏原生窗口，避免它悬在其它 UI 上
    const visible = Boolean(rect) && wantsVisible
    if (visible !== lastVisible) {
      lastVisible = visible
      void api.playerSetVisible(visible).catch(() => {
        // 后端命令未就绪：忽略
      })
    }
    if (!rect) {
      lastRect = null
      return
    }
    if (sameRect(rect, lastRect)) return
    lastRect = rect
    void api.playerSetRect(rect).catch(() => {
      // 后端命令未就绪 / 窗口重建中：下一次节拍继续尝试
    })
  }

  /** rAF 节流：一帧最多上报一次 */
  function scheduleRect() {
    if (!api.isTauri || rafId !== null) return
    rafId = window.requestAnimationFrame(flushRect)
  }

  function detachRectTarget() {
    if (rafId !== null) {
      window.cancelAnimationFrame(rafId)
      rafId = null
    }
    if (resizeObserver) {
      resizeObserver.disconnect()
      resizeObserver = null
    }
    window.removeEventListener('resize', scheduleRect)
    document.removeEventListener('scroll', scheduleRect, true)
    if (scrollEl) {
      scrollEl.removeEventListener('scroll', scheduleRect, true)
      scrollEl = null
    }
    targetEl = null
    lastRect = null
    lastVisible = false
    // 组件卸载 / 目标切换：关闭视频洞，恢复 .app-shell 完整绘制
    applyHole(null)
    setVideoRect(null)
  }

  /**
   * 开始/切换同步目标（播放器窗格）。
   * 位置变化来源：ResizeObserver、window resize、内容区滚动（capture）。
   */
  function syncRect(el: HTMLElement | null): void {
    if (!api.isTauri) return
    if (!el) {
      detachRectTarget()
      return
    }
    if (el !== targetEl) {
      detachRectTarget()
      targetEl = el
      if (typeof ResizeObserver !== 'undefined') {
        resizeObserver = new ResizeObserver(scheduleRect)
        resizeObserver.observe(el)
      }
      window.addEventListener('resize', scheduleRect)
      // 兜底：页面级滚动（含 el-scrollbar 内部滚动）同样会改变窗格位置
      document.addEventListener('scroll', scheduleRect, { passive: true, capture: true })
      scrollEl = pageScrollWrap(el)
      scrollEl?.addEventListener('scroll', scheduleRect, { passive: true, capture: true })
    } else {
      // 同一元素也可能因滚动/布局变化改了位置：强制重算
      lastRect = null
    }
    scheduleRect()
  }

  /** 停止 rect 同步（组件离开 / 全屏切换前） */
  function stopRectSync(): void {
    detachRectTarget()
  }

  /* ---------------- 状态 → 视频洞（下一帧与 rect 一起重算） ---------------- */

  watch(
    () => [state.value.status, state.value.videoReady] as const,
    () => scheduleRect(),
  )

  return {
    /** 播放器状态（只读快照，响应式） */
    state,
    /** 视频洞 / teleport 浮层矩形（CSS px / 视口坐标）；null 表示当前无原生画面 */
    videoRect,
    open,
    close,
    play,
    pause,
    togglePause,
    seek,
    setVolume,
    setMute,
    setRate,
    setVisible,
    syncRect,
    stopRectSync,
  }
}
