<!-- 播放器顶部信息条（跟随鼠标移入显隐）：
     - 左：视频分辨率（来自后端 PlayerState.width/height，未知时显示 --）
     - 中：剧情名称 + 当前第几集
     - 右：当前系统时间（精确到秒，仅显示时运行计时器） -->
<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { formatSpeed } from '@/utils/format'

const props = withDefaults(
  defineProps<{
    /** 剧集名称（详情页的片名，不是单集标题） */
    title?: string
    /** 当前集文案，如「第 3 集」 */
    episodeLabel?: string
    /** 单集标题（可选，跟在集数后面） */
    episodeTitle?: string
    /** 视频宽高（px，0 表示未知） */
    width?: number
    height?: number
    /** 当前缓冲/下载速度（字节/秒，0 表示未知） */
    speedBps?: number
    /** 当前硬解器（空 = 未知/解码器未建立；no = 软解） */
    hwdec?: string
    /** 是否已有真实视频帧：为空时用它区分「等待硬解」与真正软解 */
    videoReady?: boolean
    /** 片源帧率（0 表示未知） */
    containerFps?: number
    /** 估算帧率（containerFps 缺失时回退） */
    estimatedFps?: number
    /** 当前音轨文案（名称 + 编码；关闭时为「静音」） */
    audioLabel?: string
    /** 是否显示（由 PlayerPane 的 hover 逻辑统一控制） */
    visible?: boolean
  }>(),
  {
    title: '',
    episodeLabel: '',
    episodeTitle: '',
    width: 0,
    height: 0,
    speedBps: 0,
    hwdec: '',
    videoReady: false,
    containerFps: 0,
    estimatedFps: 0,
    audioLabel: '',
    visible: false,
  },
)

/** 分辨率文案：宽高清一不可时显示 -- */
const resolution = computed(() => {
  if (!props.width || !props.height) return '--'
  return `${props.width}×${props.height}`
})

/** 下载/缓冲速度 */
const speedText = computed(() =>
  props.speedBps > 0 ? formatSpeed(props.speedBps) : '0 KB/s',
)

/** 帧率：容器优先，否则用估算值 */
const fpsText = computed(() => {
  const fps = props.containerFps || props.estimatedFps
  if (!(fps > 0)) return ''
  return `${fps >= 100 ? fps.toFixed(0) : fps.toFixed(1)} fps`
})

/** 解码方式：mpv `hwdec-current` 为空表示「解码器未建立 / 属性暂不可用」，
    不等于软解（#9 的误报来源）。只有已有真实视频帧（videoReady）时为空才显示软解；
    否则显示「等待硬解」。显式 `no` 才是用户主动关闭硬解。 */
const decodeText = computed(() => {
  const hw = (props.hwdec || '').trim().toLowerCase()
  if (!hw) return props.videoReady ? '软解' : '等待硬解'
  if (hw === 'no') return '软解'
  return `硬解 ${hw}`
})

/** 中间第二行：当前选中的集数（单集标题与集数不同时一并显示） */
const episodeText = computed(() => {
  const label = props.episodeLabel.trim()
  const extra = props.episodeTitle.trim()
  if (!label) return extra
  if (!extra || extra === label) return label
  return `${label} · ${extra}`
})

/* ---------------- 实时时钟（精确到秒；隐藏时停表省电） ---------------- */

const clock = ref('')
let timer: number | null = null

function tick() {
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  clock.value = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`
}

function startClock() {
  tick()
  if (timer !== null) return
  timer = window.setInterval(tick, 1000)
}

function stopClock() {
  if (timer === null) return
  window.clearInterval(timer)
  timer = null
}

watch(
  () => props.visible,
  (visible) => {
    if (visible) startClock()
    else stopClock()
  },
  { immediate: true },
)

onBeforeUnmount(stopClock)
</script>

<template>
  <div class="player-topbar" :class="{ 'is-visible': visible }" aria-hidden="true">
    <div class="tb-side tb-left">
      <span class="tb-resolution">{{ resolution }}</span>
      <!-- 分辨率下方：帧率 · 解码方式 · 音轨 -->
      <span class="tb-stats">
        <template v-if="fpsText">
          <span class="tb-stat">{{ fpsText }}</span>
          <span class="tb-sep">·</span>
        </template>
        <span class="tb-stat">{{ decodeText }}</span>
        <template v-if="audioLabel">
          <span class="tb-sep">·</span>
          <span class="tb-stat">{{ audioLabel }}</span>
        </template>
      </span>
    </div>

    <div class="tb-center">
      <span v-if="title" class="tb-title ellipsis-1">{{ title }}</span>
      <span v-if="episodeText" class="tb-episode">{{ episodeText }}</span>
    </div>

    <div class="tb-side tb-right">
      <span class="tb-clock">{{ clock }}</span>
      <span class="tb-stat tb-speed">↓ {{ speedText }}</span>
    </div>
  </div>
</template>

<style scoped>
.player-topbar {
  position: absolute;
  top: 0;
  right: 0;
  left: 0;
  z-index: 2;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
  gap: var(--space-3);
  align-items: center;
  padding: var(--space-2) var(--space-4) var(--space-6);
  background: linear-gradient(to bottom, rgba(0, 0, 0, 0.62), rgba(0, 0, 0, 0));
  color: rgba(255, 255, 255, 0.92);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.22s ease;
}

.player-topbar.is-visible {
  opacity: 1;
}

.tb-side {
  display: flex;
  gap: var(--space-2);
  align-items: baseline;
  min-width: 0;
}

/* 左侧：分辨率在上，帧率 / 解码 / 音轨在下 */
.tb-left {
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
}

.tb-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 3px;
  min-width: 0;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.66);
  white-space: nowrap;
}

.tb-stat {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.1px;
}

.tb-sep {
  color: rgba(255, 255, 255, 0.4);
}

/* 右侧：系统时间在上，下载速度在下 */
.tb-right {
  flex-direction: column;
  align-items: flex-end;
  gap: 1px;
}

.tb-speed {
  font-size: 11px;
  color: rgba(255, 255, 255, 0.66);
  white-space: nowrap;
}

.tb-title {
  max-width: 100%;
  font-size: 14px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.96);
}

/* 中间第二行：选中的集数（药丸样式） */
.tb-episode {
  flex-shrink: 0;
  max-width: 100%;
  padding: 1px 7px;
  overflow: hidden;
  border-radius: var(--radius-pill);
  background: rgba(255, 255, 255, 0.16);
  color: rgba(255, 255, 255, 0.86);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tb-center {
  display: flex;
  flex-direction: column;
  gap: 1px;
  align-items: center;
  justify-content: center;
  min-width: 0;
}

.tb-resolution,
.tb-clock {
  font-family: var(--font-mono);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.2px;
  color: rgba(255, 255, 255, 0.78);
  white-space: nowrap;
}
</style>
