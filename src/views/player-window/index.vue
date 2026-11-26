<!-- 独立播放器窗口（player-window）
     - 窗口由 Rust 在 player_window_open 时创建，mpv 渲染目标直接指向本窗口 HWND；
     - 页面只做一件事：把详情页传来的完整播放上下文交给 PlayerPane（与旧详情页同一套控制条/选集/音轨字幕/倍速等 UI）；
     - 切集/重试/自动保存开关等会改动详情页状态的操作，通过 Tauri 事件回传主窗口（detail 页面处理解析与历史记录）。 -->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import * as api from '@/api'
import PlayerPane from '@/components/PlayerPane.vue'
import type { Episode, PlayerState, PlayerTrack } from '@/types'

/** 本窗口待播请求（含完整播放上下文） */
const request = ref<api.PlayerWindowRequest | null>(null)
const title = ref('')
const seriesName = ref('')
const episodeLabel = ref('')
const episodes = ref<Episode[]>([])
const activeEpisode = ref(-1)
const cover = ref<string | null>(null)
const autoSave = ref(false)
const hasMedia = ref(false)
const loading = ref(true)
const loadingText = ref('正在打开播放器…')
const errorText = ref('')

/** 轨道信息（PlayerPane 的 props 由本页维护；真实值来自 player-state 事件） */
const subtitleTracks = ref<PlayerTrack[]>([])
const audioTracks = ref<PlayerTrack[]>([])
const activeSubtitle = ref('')
const activeAudio = ref('')

const paneRef = ref<InstanceType<typeof PlayerPane> | null>(null)

const unlisteners: UnlistenFn[] = []

/** 是否已经取到过播放请求（防止重复应用） */
let appliedOnce = false

/** 把请求写进页面 props，并驱动 PlayerPane 起播 */
function applyRequest(req: api.PlayerWindowRequest) {
  request.value = req
  title.value = req.title ?? ''
  seriesName.value = req.seriesName ?? req.title ?? ''
  episodeLabel.value = req.episodeLabel ?? ''
  episodes.value = (req.episodes ?? []) as Episode[]
  activeEpisode.value = typeof req.activeEpisode === 'number' ? req.activeEpisode : -1
  cover.value = req.cover ?? null
  autoSave.value = Boolean(req.autoSave)
  hasMedia.value = true
  errorText.value = ''
  loading.value = true
  loadingText.value = '正在打开播放器…'
  void (async () => {
    try {
      await paneRef.value?.open(req.url, req.headers, req.startSec)
    } catch (e) {
      errorText.value = api.formatError(e)
    } finally {
      loading.value = false
    }
  })()
}

/* ---------------- 播放器窗口 → 主窗口事件 ---------------- */

function onSelectEpisode(index: number) {
  if (!Number.isFinite(index) || index < 0) return
  void emit('player-window-episode', { index })
}

function onPrev() {
  if (activeEpisode.value > 0) onSelectEpisode(activeEpisode.value - 1)
}

function onNext() {
  if (activeEpisode.value >= 0 && activeEpisode.value < episodes.value.length - 1) {
    onSelectEpisode(activeEpisode.value + 1)
  }
}

function onRetry() {
  void emit('player-window-retry', {})
}

function onToggleAutoSave() {
  autoSave.value = !autoSave.value
  void emit('player-window-autosave-toggle', {})
}

function onError(message: string) {
  errorText.value = message
  void emit('player-window-mpv-error', { message })
}

function onCloseWindow() {
  void api.playerWindowClose()
}

/* ---------------- 生命周期 ---------------- */

onMounted(async () => {
  // 1) 先订阅「切集/新请求」事件：窗口已存在时 Rust 直接 emit，不会走 take。
  try {
    unlisteners.push(
      await listen<api.PlayerWindowRequest>('player-window-play', (event) => {
        if (event.payload) applyRequest(event.payload)
      }),
    )
  } catch (e) {
    console.warn('[VBox] 订阅 player-window-play 失败：', e)
  }
  // 2) 轨道信息随 player-state 同步给 PlayerPane（音轨/字幕下拉列表）
  try {
    unlisteners.push(
      await listen<PlayerState>('player-state', (event) => {
        const state = event.payload
        if (!state) return
        subtitleTracks.value = state.subtitleTracks ?? []
        audioTracks.value = state.audioTracks ?? []
        activeSubtitle.value = state.activeSubtitleId ?? ''
        activeAudio.value = state.activeAudioId ?? ''
      }),
    )
  } catch (e) {
    console.warn('[VBox] 订阅 player-state 失败：', e)
  }
  // 3) 主窗口解析失败时把错误显示在播放器窗格里
  try {
    unlisteners.push(
      await listen<{ message?: string }>('player-window-parse-error', (event) => {
        errorText.value = event.payload?.message || '解析失败'
        loading.value = false
      }),
    )
  } catch (e) {
    console.warn('[VBox] 订阅 player-window-parse-error 失败：', e)
  }
  // 4) 取走首次打开窗口时暂存的待播请求（取一次即清空）
  try {
    const pending = await api.playerWindowTake()
    if (pending && !appliedOnce) {
      appliedOnce = true
      applyRequest(pending)
    }
  } catch (e) {
    errorText.value = api.formatError(e)
    loading.value = false
  }
  if (!appliedOnce) loading.value = false
})

onBeforeUnmount(() => {
  for (const off of unlisteners.splice(0)) {
    try {
      off()
    } catch {
      // 解绑失败不影响窗口销毁
    }
  }
})
</script>

<template>
  <div class="player-window-root">
    <PlayerPane
      ref="paneRef"
      window-mode
      :title="title"
      :series-name="seriesName"
      :episode-label="episodeLabel"
      :episodes="episodes"
      :active-episode="activeEpisode"
      :subtitle-tracks="subtitleTracks"
      :audio-tracks="audioTracks"
      :active-subtitle="activeSubtitle"
      :active-audio="activeAudio"
      :cover="cover"
      :has-media="hasMedia"
      :loading="loading"
      :loading-text="loadingText"
      :error-text="errorText"
      :auto-save="autoSave"
      @retry="onRetry"
      @prev="onPrev"
      @next="onNext"
      @select-episode="onSelectEpisode"
      @toggle-auto-save="onToggleAutoSave"
      @error="onError"
      @close-window="onCloseWindow"
    />
  </div>
</template>

<style>
/* 独立窗口需要整窗透明：mpv 渲染在 WebView2 下层，页面只铺交互浮层 */
html,
body,
#app {
  margin: 0;
  width: 100%;
  height: 100%;
  background: transparent !important;
  overflow: hidden;
}

.player-window-root {
  position: relative;
  width: 100vw;
  height: 100vh;
  overflow: hidden;
  background: transparent;
}
</style>
