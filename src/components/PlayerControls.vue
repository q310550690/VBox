<!-- 播放器控制条（受控组件）：悬浮在视频底部，跟随鼠标移入显隐（显隐逻辑由 PlayerPane 统一控制）
     - 第一行：可拖拽进度条（约占 90%）+ 右侧总时长；进度点上方浮动当前时间
     - 第二行：左「上一集 / 播放·暂停 / 下一集 / 重播 / 循环 / 选集 / 片头 / 片尾」
              右「下载与保存（保存本集 + 本片边下边播） / 弹幕 / 字幕 / 音轨 / 画质（Anime4K 超分） / 倍速 / 音量 / 小窗 / 弹窗 / 全屏」
     - 只负责展示与 emit；播放命令、状态、持久化都在 PlayerPane / usePlayer 内 -->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { CSSProperties } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import { formatDuration } from '@/utils/format'
import type { Episode, PlayerState, PlayerTrack, PluginInfo } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 播放器状态（进度 / 时长 / 音量 / 暂停） */
    state: PlayerState
    /** 是否显示（父组件控制 hover 显隐；隐藏时淡出且不拦截鼠标） */
    visible?: boolean
    /** 是否处于窗格全屏 */
    fullscreen?: boolean
    /** 是否处于小窗态 */
    mini?: boolean
  /** 独立播放器窗口内无「页内小窗」概念，隐藏小窗按钮 */
  allowMini?: boolean
  /** 独立播放器窗口内隐藏「弹窗播放」（本身已是独立窗口） */
  allowPopOut?: boolean
    /** 禁用全部控件（error / opening 时） */
    disabled?: boolean
    /** 循环播放开关状态 */
    loop?: boolean
    /** 弹幕开关状态 */
    danmaku?: boolean
    /** 当前线路的选集；空数组时「选集」按钮置灰 */
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
    /** 画质增强档位：off / lite / standard / strong（播放器页面的「画质」开关） */
    enhance?: string
    /** 已安装插件（GLSL 着色器 / lavfi 滤镜），画质浮层里快速启停 */
    pluginItems?: PluginInfo[]
    /** 已设置的片头位置（秒）；null/undefined = 未设置 */
    introSec?: number | null
    /** 已设置的片尾位置（秒）；null/undefined = 未设置 */
    outroSec?: number | null
    /** 当前是否为 HLS 代理会话（可「保存本集」） */
    canSave?: boolean
    /** 保存状态：idle / saving / paused / done */
    saveState?: 'idle' | 'saving' | 'paused' | 'done'
    /** 单片「边下边播」开关状态（详情页算好：本片覆盖优先，否则跟随全局设置） */
    autoSave?: boolean
  }>(),
  {
    visible: true,
    fullscreen: false,
    mini: false,
  allowMini: true,
    allowPopOut: true,
    disabled: false,
    loop: false,
    danmaku: false,
    episodes: () => [],
    activeEpisode: -1,
    subtitleTracks: () => [],
    audioTracks: () => [],
    activeSubtitle: '',
    activeAudio: '',
    enhance: 'off',
    pluginItems: () => [],
    introSec: null,
    outroSec: null,
    canSave: false,
    saveState: 'idle',
    autoSave: false,
  },
)

const emit = defineEmits<{
  (e: 'toggle'): void
  (e: 'prev'): void
  (e: 'next'): void
  (e: 'replay'): void
  (e: 'seek', sec: number): void
  (e: 'volume', value: number): void
  (e: 'mute'): void
  (e: 'fullscreen'): void
  (e: 'select-episode', index: number): void
  (e: 'toggle-loop'): void
  (e: 'toggle-danmaku'): void
  (e: 'select-subtitle', id: string): void
  (e: 'load-subtitle'): void
  (e: 'search-online-subtitle'): void
  (e: 'select-audio', id: string): void
  (e: 'enhance', mode: string): void
  (e: 'select-plugin-preset', id: string, preset: string): void
  (e: 'toggle-plugin', id: string, enabled: boolean): void
  (e: 'rate', value: number): void
  (e: 'toggle-mini'): void
  (e: 'pop-out'): void
  (e: 'set-intro'): void
  (e: 'set-outro'): void
  (e: 'clear-intro'): void
  (e: 'clear-outro'): void
  (e: 'save-episode'): void
  (e: 'toggle-auto-save'): void
  (e: 'panel-toggle', open: boolean): void
}>()

/* ---------------- 窄屏折叠：左右各一个「更多」 ---------------- */

/** 控制条内容宽度低于该值时，开始把内侧按钮收进「更多」 */
const COMPACT_BREAKPOINT = 620
/** 一个按钮位的大致宽度（按钮 30px + 间距），用于估算折叠数量 */
const COLLAPSE_STEP = 32
/** 左侧从右往左收起：片尾 → 片头 → 循环 → 重播（播放/上一集/下一集/选集保留） */
const LEFT_COLLAPSE_ORDER = ['outro', 'intro', 'loop', 'replay'] as const
/** 右侧按原有顺序从内侧收起，音量/全屏始终保留 */
const RIGHT_COLLAPSE_ORDER = ['danmaku', 'enhance', 'rate', 'mini'] as const
type LeftCollapseKey = (typeof LEFT_COLLAPSE_ORDER)[number]
type RightCollapseKey = (typeof RIGHT_COLLAPSE_ORDER)[number]

const rootRef = ref<HTMLElement | null>(null)
const barWidth = ref(0)
const compact = computed(() => barWidth.value > 0 && barWidth.value < COMPACT_BREAKPOINT)
const moreLeftOpen = ref(false)
const moreRightOpen = ref(false)
/** 「更多」菜单内联展开的二级选项，避免关闭菜单后弹层锚点失效导致漂移 */
const moreEnhanceOpen = ref(false)
const moreRateOpen = ref(false)

/** 估算需要从左右两侧各收起多少个按钮；同时为两个「更多」按钮预留位置 */
const collapseLevel = computed(() => {
  if (!compact.value) return 0
  const overflow = COMPACT_BREAKPOINT - barWidth.value
  return Math.max(2, Math.ceil(1 + overflow / (2 * COLLAPSE_STEP)))
})
const leftCollapsed = computed(
  () => new Set(LEFT_COLLAPSE_ORDER.slice(0, Math.min(collapseLevel.value, LEFT_COLLAPSE_ORDER.length))),
)
const rightCollapsed = computed(
  () => new Set(RIGHT_COLLAPSE_ORDER.slice(0, Math.min(collapseLevel.value, RIGHT_COLLAPSE_ORDER.length))),
)
const isLeftCollapsed = (key: LeftCollapseKey) => leftCollapsed.value.has(key)
const isRightCollapsed = (key: RightCollapseKey) => rightCollapsed.value.has(key)

let resizeObserver: ResizeObserver | null = null

onMounted(() => {
  if (!rootRef.value || typeof ResizeObserver === 'undefined') return
  resizeObserver = new ResizeObserver((entries) => {
    barWidth.value = entries[0]?.contentRect.width ?? 0
  })
  resizeObserver.observe(rootRef.value)
})

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})

/* ---------------- 状态派生 ---------------- */

/** 保存按钮状态派生：图标 / 提示 / 是否旋转 */
const saveIcon = computed(() => {
  if (props.saveState === 'done') return 'check-circle'
  if (props.saveState === 'paused') return 'play'
  if (props.saveState === 'saving') return 'refresh'
  return 'download'
})
const saveTip = computed(() => {
  if (props.saveState === 'saving') return '保存中…（点击暂停）'
  if (props.saveState === 'paused') return '已暂停（点击继续）'
  if (props.saveState === 'done') return '已保存'
  return '保存本集'
})

const duration = computed(() => (props.state.durationSec > 0 ? props.state.durationSec : 0))
const position = computed(() => {
  const pos = Math.max(0, props.state.positionSec)
  return duration.value > 0 ? Math.min(pos, duration.value) : pos
})
const percent = computed(() => {
  if (duration.value <= 0) return 0
  return Math.min(100, Math.max(0, (position.value / duration.value) * 100))
})
/** 缓存进度：当前位置 + 已缓冲秒数；始终不早于播放进度（视觉上缓存层包住 fill） */
const bufferPercent = computed(() => {
  if (duration.value <= 0) return 0
  const cacheSec = Math.max(0, props.state.cacheSec || 0)
  const cached = Math.min(100, ((position.value + cacheSec) / duration.value) * 100)
  return Math.max(percent.value, Math.max(0, cached))
})
const isPlaying = computed(() => props.state.status === 'playing' && !props.state.paused)
const totalLabel = computed(() => (duration.value > 0 ? formatDuration(duration.value) : '--:--'))

const canPrev = computed(() => props.activeEpisode > 0)
const canNext = computed(
  () => props.activeEpisode >= 0 && props.activeEpisode < props.episodes.length - 1,
)
const hasEpisodes = computed(() => props.episodes.length > 0)

/** 音量图标随音量大小变化（静音 / 小 / 中 / 大） */
const volumeIcon = computed<SolarIconName>(() => {
  if (props.state.muted || props.state.volume <= 0) return 'muted'
  if (props.state.volume < 34) return 'volume'
  if (props.state.volume < 67) return 'volume-small'
  return 'volume-loud'
})

/** 音量滑杆双向绑定（静音时显示 0） */
const volumeValue = computed({
  get: () => (props.state.muted ? 0 : Math.round(props.state.volume)),
  set: (value: number) => emit('volume', value),
})

const introLabel = computed(() =>
  typeof props.introSec === 'number' ? formatDuration(props.introSec) : '未设置',
)
const outroLabel = computed(() =>
  typeof props.outroSec === 'number' ? formatDuration(props.outroSec) : '未设置',
)
/** 是否已设置片头 / 片尾（决定是否显示时间 + 取消按钮） */
const introSet = computed(() => typeof props.introSec === 'number')
const outroSet = computed(() => typeof props.outroSec === 'number')

/* ---------------- 进度条：hover 提示 + 拖拽 seek ---------------- */

const trackRef = ref<HTMLElement | null>(null)
const dragging = ref(false)
const dragPercent = ref(0)
/** 鼠标悬停位置（0-100）；null = 未悬停 */
const hoverPercent = ref<number | null>(null)

function percentAt(clientX: number): number {
  const rect = trackRef.value?.getBoundingClientRect()
  if (!rect || rect.width <= 0) return 0
  return Math.min(100, Math.max(0, ((clientX - rect.left) / rect.width) * 100))
}

function onTrackDown(event: PointerEvent) {
  if (props.disabled || duration.value <= 0) return
  dragging.value = true
  dragPercent.value = percentAt(event.clientX)
  hoverPercent.value = dragPercent.value
    ; (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId)
}

function onTrackMove(event: PointerEvent) {
  const percentValue = percentAt(event.clientX)
  hoverPercent.value = percentValue
  if (dragging.value) dragPercent.value = percentValue
}

function onTrackUp(event: PointerEvent) {
  if (!dragging.value) return
  const percentValue = percentAt(event.clientX)
  dragging.value = false
    ; (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId)
  emit('seek', (percentValue / 100) * duration.value)
}

function onTrackLeave() {
  if (!dragging.value) hoverPercent.value = null
}

/** 滑块位置：拖拽时跟手，否则跟随播放进度 */
const thumbPercent = computed(() => (dragging.value ? dragPercent.value : percent.value))
/** 滑块本身就是当前播放时间（拖拽时显示目标时间） */
const thumbLabel = computed(() => {
  if (duration.value <= 0) return '--:--'
  return formatDuration((thumbPercent.value / 100) * duration.value)
})
/** 悬停提示：鼠标在进度条上时显示该位置的时间 */
const hoverLabel = computed(() => {
  if (hoverPercent.value === null || duration.value <= 0) return ''
  return formatDuration((hoverPercent.value / 100) * duration.value)
})
/** 两端向内收，避免滑块 / 提示被裁掉 */
const thumbLeft = computed(() => `clamp(24px, ${thumbPercent.value}%, calc(100% - 24px))`)
const hoverLeft = computed(() =>
  hoverPercent.value === null
    ? '0'
    : `clamp(30px, ${hoverPercent.value}%, calc(100% - 30px))`,
)

/* ---------------- 弹层开关 ---------------- */

const episodesOpen = ref(false)
const subtitleOpen = ref(false)
const audioOpen = ref(false)
const enhanceOpen = ref(false)
const rateOpen = ref(false)
const volumeOpen = ref(false)
const downloadOpen = ref(false)

/** 退出窄屏时收起「更多」 */
watch(compact, (isCompact) => {
  if (!isCompact) {
    moreLeftOpen.value = false
    moreRightOpen.value = false
  }
})
watch(
  () => leftCollapsed.value.size,
  (size) => {
    if (!size) moreLeftOpen.value = false
  },
)
watch(
  () => rightCollapsed.value.size,
  (size) => {
    if (!size) moreRightOpen.value = false
  },
)

/** 二级浮层互斥：打开一个时收起另一个 */
watch(moreEnhanceOpen, (open) => {
  if (open) moreRateOpen.value = false
})
watch(moreRateOpen, (open) => {
  if (open) moreEnhanceOpen.value = false
})

/** 打开「更多」时先关掉已展开的弹层；关闭时重置二级浮层 */
watch(moreRightOpen, (open) => {
  if (open) {
    enhanceOpen.value = false
    rateOpen.value = false
    moreEnhanceOpen.value = false
    moreRateOpen.value = false
  } else {
    moreEnhanceOpen.value = false
    moreRateOpen.value = false
  }
})

/** 弹层统一深色半透明（inline style 兜底，避免 popper 默认白底） */
const POPOVER_STYLE: CSSProperties = {
  padding: 'var(--space-2)',
  border: '1px solid rgba(255, 255, 255, 0.14)',
  borderRadius: 'var(--radius-md)',
  background: 'rgba(18, 18, 22, 0.72)',
  color: 'rgba(255, 255, 255, 0.9)',
  boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
  backdropFilter: 'blur(14px) saturate(140%)',
  WebkitBackdropFilter: 'blur(14px) saturate(140%)',
  minWidth: 0,
}

/** 有弹层打开时通知父组件保持控制条常显（弹层被 teleport 到 body，鼠标会离开窗格） */
const anyPanelOpen = computed(
  () =>
    episodesOpen.value ||
    subtitleOpen.value ||
    audioOpen.value ||
    enhanceOpen.value ||
    rateOpen.value ||
    volumeOpen.value ||
    downloadOpen.value ||
    moreLeftOpen.value ||
    moreRightOpen.value,
)

watch(anyPanelOpen, (open) => emit('panel-toggle', open))

function onSelectEpisode(index: number) {
  emit('select-episode', index)
  episodesOpen.value = false
}

function onSelectSubtitle(id: string) {
  emit('select-subtitle', id)
  subtitleOpen.value = false
}

/** 加载本地外挂字幕：关闭弹层并交给父组件开原生文件对话框 */
function onLoadSubtitle() {
  emit('load-subtitle')
  subtitleOpen.value = false
}

/** 在线搜索字幕：关闭弹层并交给父组件打开搜索弹窗 */
function onSearchOnlineSubtitle() {
  emit('search-online-subtitle')
  subtitleOpen.value = false
}

function onSelectAudio(id: string) {
  emit('select-audio', id)
  audioOpen.value = false
}

/* ---------------- 画质增强（Anime4K 超分） ---------------- */

/** 当前档位（后端回填前默认 off；仅用于按钮高亮与标题）。 */
const currentEnhance = computed(() => props.enhance || 'off')
const enhanceLabel = computed(() => {
  if (currentEnhance.value === 'lite') return '轻量'
  if (currentEnhance.value === 'standard') return '标准'
  if (currentEnhance.value === 'strong') return '强力'
  return '关闭'
})

/* ---------------- 插件（按用途分组） ---------------- */

type PluginPurpose = 'upscale' | 'hdr' | 'sharpen' | 'repair' | 'style' | 'other'

const PURPOSE_ORDER: { key: PluginPurpose; title: string; hint: string }[] = [
  { key: 'upscale', title: '超分', hint: '单选' },
  { key: 'hdr', title: 'HDR / 色彩', hint: '单选' },
  { key: 'sharpen', title: '锐化', hint: '可叠加' },
  { key: 'repair', title: '修复', hint: '可叠加' },
  { key: 'style', title: '调色 / 风格', hint: '可叠加' },
  { key: 'other', title: '其它', hint: '' },
]

/** 插件用途：取第一个带 category 的贡献项。 */
function pluginPurpose(plugin: PluginInfo): PluginPurpose {
  const entries = [
    ...plugin.shaders,
    ...plugin.videoFilters,
    ...plugin.audioFilters,
  ]
  const category = entries.find((entry) => entry.category)?.category
  if (
    category === 'upscale' ||
    category === 'hdr' ||
    category === 'sharpen' ||
    category === 'repair' ||
    category === 'style'
  ) {
    return category
  }
  return 'other'
}

/** 多预设插件（Anime4K / FSRCNNX 等）展开二级预设选择。 */
const presetOpenId = ref<string | null>(null)

function supportsPresets(plugin: PluginInfo) {
  return plugin.shaders.length > 1
}

function onPluginClick(plugin: PluginInfo) {
  if (supportsPresets(plugin)) {
    presetOpenId.value = presetOpenId.value === plugin.id ? null : plugin.id
    return
  }
  emit('toggle-plugin', plugin.id, !plugin.enabled)
}

function onPickPreset(plugin: PluginInfo, preset: string) {
  // 选择预设后保持子集展开，方便连续切换；再次点击当前预设 = 关闭该插件。
  const close = plugin.enabled && plugin.preset === preset
  emit('select-plugin-preset', plugin.id, close ? '' : preset)
}

function gpuLoadLabel(load?: string) {
  if (load === 'high') return '高负载'
  if (load === 'medium') return '中负载'
  if (load === 'low') return '低负载'
  return ''
}

function pluginSubLabel(plugin: PluginInfo) {
  if (supportsPresets(plugin)) {
    const current = plugin.shaders.find((shader) => shader.id === plugin.preset)
    return current?.name || (plugin.enabled ? '选择预设' : '预设')
  }
  return plugin.type === 'shader' ? '着色器' : plugin.type === 'filter' ? '滤镜' : '组合'
}

/** 按用途分组，只保留有插件的组。 */
const pluginGroups = computed(() => {
  const buckets = new Map<PluginPurpose, PluginInfo[]>()
  for (const plugin of props.pluginItems) {
    const purpose = pluginPurpose(plugin)
    const list = buckets.get(purpose) ?? []
    list.push(plugin)
    buckets.set(purpose, list)
  }
  return PURPOSE_ORDER.map((group) => ({
    ...group,
    items: buckets.get(group.key) ?? [],
  })).filter((group) => group.items.length > 0)
})

/* ---------------- 倍速 ---------------- */

/** 可选倍速（mpv 支持 0.25–4.0，这里按从快到慢排列） */
const RATE_OPTIONS = [2, 1.5, 1.25, 1, 0.75, 0.5] as const

/** 当前倍速（做 0.01 容差，避免浮点比较失败） */
const currentRate = computed(() => {
  const value = props.state.rate || 1
  return value.toFixed(2)
})

function onSelectRate(value: number) {
  emit('rate', value)
  rateOpen.value = false
}

function episodeText(episode: Episode, index: number): string {
  return episode.title || `第${index + 1}集`
}
</script>

<template>
  <div
    ref="rootRef"
    class="player-controls"
    :class="{ 'is-visible': visible, 'is-disabled': disabled, 'is-mini': mini }"
    role="toolbar"
    aria-label="播放控制条"
    @click.stop
    @mousedown.stop
    @dblclick.stop
  >
    <!-- 第一行：进度条（约 90%）+ 总时长 -->
    <div class="row progress-row">
      <div
        ref="trackRef"
        class="progress"
        :class="{ 'is-dragging': dragging, 'is-disabled': disabled || duration <= 0 }"
        role="slider"
        aria-label="播放进度"
        :aria-valuemin="0"
        :aria-valuemax="100"
        :aria-valuenow="Math.round(percent)"
        tabindex="0"
        @pointerdown="onTrackDown"
        @pointermove="onTrackMove"
        @pointerup="onTrackUp"
        @pointercancel="onTrackUp"
        @pointerleave="onTrackLeave"
      >
        <div class="progress-track">
          <!-- 缓存进度层：位于轨道背景之上、播放进度之下（只做展示，不拦截指针） -->
          <div
            class="progress-buffer"
            :style="{ width: `${bufferPercent}%` }"
          />
          <div
            class="progress-fill"
            :style="{ width: `${percent}%` }"
          />
        </div>
        <!-- 滑块 = 当前播放时间（10000px 圆角胶囊） -->
        <div
          class="progress-thumb"
          :style="{ left: thumbLeft }"
        >{{ thumbLabel }}</div>
        <!-- hover 时在光标位置显示该处时间 -->
        <div
          v-if="hoverPercent !== null"
          class="progress-tip"
          :style="{ left: hoverLeft }"
        >
          {{ hoverLabel }}
        </div>
      </div>

      <span class="time-total">{{ totalLabel }}</span>
    </div>

    <!-- 第二行：左操作区 / 右操作区 -->
    <div class="row buttons-row">
      <div class="btn-group btn-group-left">
        <!-- 左侧按钮被收起时，更多按钮贴在本组最右侧 -->
        <el-popover
          v-if="leftCollapsed.size > 0"
          v-model:visible="moreLeftOpen"
          placement="top-end"
          :width="208"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn more-left-btn"
              :disabled="disabled"
              aria-label="更多操作"
              title="更多操作"
            >
              <SolarIcon name="menu-dots" :size="18" />
            </button>
          </template>
          <div class="pop-head">
            <span>更多操作</span>
          </div>
          <div class="more-menu">
            <button
              v-if="isLeftCollapsed('replay')"
              type="button"
              class="track-item"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('replay')"
            >
              <SolarIcon name="restart" :size="14" />
              <span class="track-text">重播</span>
            </button>
            <button
              v-if="isLeftCollapsed('loop')"
              type="button"
              class="track-item"
              :class="{ 'is-current': loop }"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('toggle-loop')"
            >
              <SolarIcon name="repeat" :size="14" />
              <span class="track-text">{{ loop ? '关闭循环播放' : '开启循环播放' }}</span>
            </button>
            <button
              v-if="isLeftCollapsed('intro')"
              type="button"
              class="track-item"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('set-intro')"
            >
              <SolarIcon name="flag" :size="14" />
              <span class="track-text">{{ introSet ? '重新设置片头' : '设置片头' }}</span>
              <span class="track-lang">{{ introLabel }}</span>
            </button>
            <button
              v-if="isLeftCollapsed('intro') && introSet"
              type="button"
              class="track-item"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('clear-intro')"
            >
              <SolarIcon name="close" :size="14" />
              <span class="track-text">清除片头</span>
            </button>
            <button
              v-if="isLeftCollapsed('outro')"
              type="button"
              class="track-item"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('set-outro')"
            >
              <SolarIcon name="flag-2" :size="14" />
              <span class="track-text">{{ outroSet ? '重新设置片尾' : '设置片尾' }}</span>
              <span class="track-lang">{{ outroLabel }}</span>
            </button>
            <button
              v-if="isLeftCollapsed('outro') && outroSet"
              type="button"
              class="track-item"
              :disabled="disabled"
              @click="moreLeftOpen = false; emit('clear-outro')"
            >
              <SolarIcon name="close" :size="14" />
              <span class="track-text">清除片尾</span>
            </button>
          </div>
        </el-popover>

        <!-- 上一集 -->
        <el-tooltip
          content="上一集"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :disabled="disabled || !canPrev"
            aria-label="上一集"
            @click="emit('prev')"
          >
            <SolarIcon
              name="skip-previous"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 播放 / 暂停 -->
        <button
          type="button"
          class="pc-btn pc-btn-main"
          :disabled="disabled"
          :aria-label="isPlaying ? '暂停' : '播放'"
          :title="isPlaying ? '暂停' : '播放'"
          @click="emit('toggle')"
        >
          <SolarIcon
            v-if="isPlaying"
            name="pause"
            :size="22"
          />
          <SolarIcon
            v-else
            name="play"
            :size="22"
          />
        </button>

        <!-- 下一集 -->
        <el-tooltip
          content="下一集"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :disabled="disabled || !canNext"
            aria-label="下一集"
            @click="emit('next')"
          >
            <SolarIcon
              name="skip-next"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 重播 -->
        <el-tooltip
          v-if="!isLeftCollapsed('replay')"
          content="重播"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :disabled="disabled"
            aria-label="重播"
            @click="emit('replay')"
          >
            <SolarIcon
              name="restart"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 循环播放 -->
        <el-tooltip
          v-if="!isLeftCollapsed('loop')"
          :content="loop ? '循环播放：开' : '循环播放：关'"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :class="{ 'is-active': loop }"
            :disabled="disabled"
            :aria-pressed="loop"
            aria-label="循环播放"
            @click="emit('toggle-loop')"
          >
            <SolarIcon
              name="repeat"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 选集（弹层） -->
        <el-popover
          v-model:visible="episodesOpen"
          placement="top"
          :width="260"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': episodesOpen }"
              :disabled="disabled || !hasEpisodes"
              aria-label="选集"
              title="选集"
            >
              <SolarIcon
                name="playlist"
                :size="18"
              />
            </button>
          </template>
          <div class="pop-head">
            <span>选集</span>
            <span class="pop-count">共 {{ episodes.length }} 集</span>
          </div>
          <el-scrollbar max-height="240px">
            <div class="ep-list">
              <button
                v-for="(episode, index) in episodes"
                :key="index"
                type="button"
                class="ep-item"
                :class="{ 'is-current': index === activeEpisode }"
                @click="onSelectEpisode(index)"
              >
                <span class="ep-item-text ellipsis-1">{{ episodeText(episode, index) }}</span>
              </button>
              <p
                v-if="!episodes.length"
                class="pop-empty"
              >该线路暂无可播放剧集</p>
            </div>
          </el-scrollbar>
        </el-popover>

        <!-- 片头 / 片尾：点击图标即设为当前时间；已设置时显示时间，x 取消 -->
        <div
          v-if="!isLeftCollapsed('intro')"
          class="mark-group"
          :class="{ 'is-disabled': disabled }"
          role="button"
          tabindex="0"
          :aria-label="introSet ? '重新设置片头为当前时间' : '设置当前时间为片头'"
          :title="introSet ? `重新设置为当前时间（当前 ${introLabel}）` : '设置当前时间为片头'"
          @click="!disabled && emit('set-intro')"
          @keydown.enter.prevent="!disabled && emit('set-intro')"
          @keydown.space.prevent="!disabled && emit('set-intro')"
        >
          <span class="mark-icon" :class="{ 'is-active': introSet }">
            <SolarIcon name="flag" :size="18" />
          </span>
          <template v-if="introSet">
            <span class="mark-time">{{ introLabel }}</span>
            <button
              type="button"
              class="mark-clear"
              :disabled="disabled"
              aria-label="取消片头"
              title="取消片头"
              @click.stop="emit('clear-intro')"
            >
              <SolarIcon name="close" :size="12" />
            </button>
          </template>
        </div>

        <!-- 片尾 -->
        <div
          v-if="!isLeftCollapsed('outro')"
          class="mark-group mark-group--outro"
          :class="{ 'is-disabled': disabled }"
          role="button"
          tabindex="0"
          :aria-label="outroSet ? '重新设置片尾为当前时间' : '设置当前时间为片尾'"
          :title="outroSet ? `重新设置为当前时间（当前 ${outroLabel}）` : '设置当前时间为片尾'"
          @click="!disabled && emit('set-outro')"
          @keydown.enter.prevent="!disabled && emit('set-outro')"
          @keydown.space.prevent="!disabled && emit('set-outro')"
        >
          <span class="mark-icon" :class="{ 'is-active': outroSet }">
            <SolarIcon name="flag-2" :size="18" />
          </span>
          <template v-if="outroSet">
            <span class="mark-time">{{ outroLabel }}</span>
            <button
              type="button"
              class="mark-clear"
              :disabled="disabled"
              aria-label="取消片尾"
              title="取消片尾"
              @click.stop="emit('clear-outro')"
            >
              <SolarIcon name="close" :size="12" />
            </button>
          </template>
        </div>
      </div>

      <span class="btn-spacer" />

      <div class="btn-group btn-group-right">
        <!-- 下载 / 保存：把「保存本集」（一次性动作）和「本片边下边播」（本片策略）合并到一个入口，
             避免两个下载系图标并排、用户分不清哪个是「现在存」哪个是「边看边自动存」。 -->
        <el-popover
          v-if="hasEpisodes || canSave"
          v-model:visible="downloadOpen"
          placement="top"
          :width="252"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': autoSave }"
              :disabled="disabled"
              :aria-label="`下载与保存（${saveTip}）`"
              :title="`下载与保存（${saveTip}）`"
            >
              <SolarIcon
                :name="saveIcon"
                :size="18"
                :class="{ 'is-spinning': saveState === 'saving' }"
              />
            </button>
          </template>

          <div class="pop-head">
            <span>下载与保存</span>
            <span class="pop-count">{{ activeEpisode >= 0 ? `第 ${activeEpisode + 1} 集` : '当前集' }}</span>
          </div>

          <!-- 动作：立即导出当前集（HLS 复用已缓存分片；其他格式由 ffmpeg 重封装） -->
          <button
            v-if="canSave"
            type="button"
            class="track-item"
            :disabled="disabled"
            @click="emit('save-episode')"
          >
            <SolarIcon
              :name="saveIcon"
              :size="14"
              :class="{ 'is-spinning': saveState === 'saving' }"
            />
            <span class="track-text ellipsis-1">{{ saveTip }}</span>
          </button>
          <p v-else class="pop-empty">当前播放源不支持直接保存（本地文件）</p>

          <!-- 策略：本片边下边播（仅本片，覆盖设置页全局「播放时自动保存」） -->
          <div v-if="hasEpisodes" class="autosave-row">
            <div class="autosave-text">
              <span class="autosave-title">本片边下边播</span>
              <span class="autosave-desc">看剧时自动保存每一集 · 仅本片，覆盖设置页「播放时自动保存」</span>
            </div>
            <el-switch
              class="autosave-switch"
              :model-value="autoSave"
              size="small"
              :disabled="disabled"
              aria-label="本片边下边播"
              @update:model-value="emit('toggle-auto-save')"
            />
          </div>
        </el-popover>

        <!-- 弹幕开关 -->
        <el-tooltip
          v-if="!isRightCollapsed('danmaku')"
          :content="danmaku ? '关闭弹幕' : '开启弹幕'"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :class="{ 'is-active': danmaku }"
            :disabled="disabled"
            :aria-pressed="danmaku"
            aria-label="弹幕开关"
            @click="emit('toggle-danmaku')"
          >
            <SolarIcon
              name="chat-round-line"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 字幕 -->
        <el-popover
          v-model:visible="subtitleOpen"
          placement="top"
          :width="220"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': Boolean(activeSubtitle) }"
              aria-label="字幕"
              title="字幕"
            >
              <SolarIcon
                name="subtitles"
                :size="18"
              />
            </button>
          </template>
          <div class="pop-head">
            <span>字幕</span>
            <span class="pop-count">{{ subtitleTracks.length ? `${subtitleTracks.length} 条` : '默认' }}</span>
          </div>
          <el-scrollbar max-height="240px">
            <div class="track-list">
              <button
                type="button"
                class="track-item"
                :class="{ 'is-current': !activeSubtitle }"
                @click="onSelectSubtitle('')"
              >
                <SolarIcon
                  v-if="!activeSubtitle"
                  name="check-circle"
                  :size="14"
                />
                <span class="track-text ellipsis-1">关闭字幕</span>
              </button>
              <button
                v-for="track in subtitleTracks"
                :key="track.id"
                type="button"
                class="track-item"
                :class="{ 'is-current': track.id === activeSubtitle }"
                @click="onSelectSubtitle(track.id)"
              >
                <SolarIcon
                  v-if="track.id === activeSubtitle"
                  name="check-circle"
                  :size="14"
                />
                <span class="track-text ellipsis-1">{{ track.label }}</span>
                <span
                  v-if="track.lang"
                  class="track-lang"
                >{{ track.lang }}</span>
                <span
                  v-if="track.external"
                  class="track-tag"
                >外挂</span>
              </button>
              <p
                v-if="!subtitleTracks.length"
                class="pop-empty"
              >暂无可用字幕</p>
            </div>
          </el-scrollbar>
          <button
            type="button"
            class="track-add"
            :disabled="disabled"
            title="选择本地字幕文件（srt / ass / vtt 等）"
            @click="onLoadSubtitle"
          >
            <SolarIcon
              name="add"
              :size="14"
            />
            <span class="track-text">加载本地字幕…</span>
          </button>
          <button
            type="button"
            class="track-add"
            :disabled="disabled"
            title="在线搜索字幕（SubHD）"
            @click="onSearchOnlineSubtitle"
          >
            <SolarIcon
              name="magnifier"
              :size="14"
            />
            <span class="track-text">在线搜索字幕…</span>
          </button>
        </el-popover>

        <!-- 音轨 -->
        <el-popover
          v-model:visible="audioOpen"
          placement="top"
          :width="220"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': Boolean(activeAudio) }"
              aria-label="音轨"
              title="音轨"
            >
              <SolarIcon
                name="music-notes"
                :size="18"
              />
            </button>
          </template>
          <div class="pop-head">
            <span>音轨</span>
            <span class="pop-count">{{ audioTracks.length ? `${audioTracks.length} 条` : '默认' }}</span>
          </div>
          <el-scrollbar max-height="240px">
            <div class="track-list">
              <button
                v-for="track in audioTracks"
                :key="track.id"
                type="button"
                class="track-item"
                :class="{ 'is-current': track.id === activeAudio }"
                @click="onSelectAudio(track.id)"
              >
                <SolarIcon
                  v-if="track.id === activeAudio"
                  name="check-circle"
                  :size="14"
                />
                <span class="track-text ellipsis-1">{{ track.label }}</span>
                <span
                  v-if="track.lang"
                  class="track-lang"
                >{{ track.lang }}</span>
              </button>
              <p
                v-if="!audioTracks.length"
                class="pop-empty"
              >暂无其它音轨</p>
            </div>
          </el-scrollbar>
        </el-popover>

        <!-- 画质增强（Anime4K 超分）：开在播放器页面，不放进设置页 -->
        <el-popover
          v-model:visible="enhanceOpen"
          placement="top"
          :width="230"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': currentEnhance !== 'off', 'pc-collapsed': isRightCollapsed('enhance') }"
              :disabled="disabled"
              aria-label="画质增强"
              :title="`画质增强（${enhanceLabel}）`"
            >
              <SolarIcon
                name="high-quality"
                :size="18"
              />
            </button>
          </template>
          <template v-if="props.pluginItems.length">
            <div class="pop-head pop-head--plugins">
              <span>插件</span>
              <span class="pop-count">{{ props.pluginItems.filter((item) => item.enabled).length }} 个启用</span>
            </div>
            <div class="track-list track-list--plugins">
              <section
                v-for="group in pluginGroups"
                :key="group.key"
                class="plugin-group"
              >
                <div class="plugin-group-head">
                  <span>{{ group.title }}</span>
                  <span v-if="group.hint" class="plugin-group-hint">{{ group.hint }}</span>
                </div>
                <div v-for="plugin in group.items" :key="plugin.id" class="plugin-entry">
                  <button
                    type="button"
                    class="track-item"
                    :class="{ 'is-current': plugin.enabled }"
                    :title="plugin.error || plugin.description || plugin.name"
                    @click="onPluginClick(plugin)"
                  >
                    <SolarIcon
                      v-if="plugin.enabled"
                      name="check-circle"
                      :size="14"
                    />
                    <span class="track-text ellipsis-1">{{ plugin.name || plugin.id }}</span>
                    <span
                      v-if="plugin.videoFilters.length > 0"
                      class="plugin-decode is-soft"
                      title="启用后会切到 CPU 软解（lavfi 视频滤镜）"
                    >
                      软解
                    </span>
                    <span class="track-lang">{{ pluginSubLabel(plugin) }}</span>
                  </button>
                  <div v-if="presetOpenId === plugin.id" class="plugin-presets">
                    <button
                      v-for="shader in plugin.shaders"
                      :key="shader.id"
                      type="button"
                      class="track-item plugin-preset"
                      :class="{ 'is-current': plugin.enabled && plugin.preset === shader.id }"
                      :title="shader.gpuLoad ? `GPU 负载：${shader.gpuLoad}` : ''"
                      @click="onPickPreset(plugin, shader.id)"
                    >
                      <SolarIcon
                        v-if="plugin.enabled && plugin.preset === shader.id"
                        name="check-circle"
                        :size="14"
                      />
                      <span class="track-text ellipsis-1">{{ shader.name || shader.id }}</span>
                      <span v-if="gpuLoadLabel(shader.gpuLoad)" class="track-lang">
                        {{ gpuLoadLabel(shader.gpuLoad) }}
                      </span>
                    </button>
                  </div>
                </div>
              </section>
            </div>
          </template>
        </el-popover>

        <!-- 倍速 -->
        <el-popover
          v-model:visible="rateOpen"
          placement="top"
          :width="140"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn rate-btn"
              :class="{ 'is-active': Math.abs(state.rate - 1) > 0.01, 'pc-collapsed': isRightCollapsed('rate') }"
              :disabled="disabled"
              :aria-label="`播放倍速 ${currentRate}x`"
              title="播放倍速"
            >
              <SolarIcon
                name="playback-speed"
                :size="18"
              />
              <span class="rate-text">{{ Number(currentRate) }}x</span>
            </button>
          </template>
          <div class="pop-head">
            <span>播放倍速</span>
            <span class="pop-count">{{ Number(currentRate) }}x</span>
          </div>
          <div class="track-list">
            <button
              v-for="rate in RATE_OPTIONS"
              :key="rate"
              type="button"
              class="track-item"
              :class="{ 'is-current': Math.abs(state.rate - rate) < 0.01 }"
              @click="onSelectRate(rate)"
            >
              <SolarIcon
                v-if="Math.abs(state.rate - rate) < 0.01"
                name="check-circle"
                :size="14"
              />
              <span class="track-text ellipsis-1">{{ rate }}x</span>
            </button>
          </div>
        </el-popover>

        <!-- 音量：鼠标移入展开滑杆，点击图标静音/取消静音 -->
        <el-popover
          v-model:visible="volumeOpen"
          placement="top"
          :width="34"
          trigger="hover"
          :show-after="80"
          :hide-after="120"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover player-popover-volume"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn"
              :class="{ 'is-active': volumeOpen || state.muted }"
              :disabled="disabled"
              :aria-label="state.muted ? '取消静音' : '静音'"
              :title="state.muted ? '取消静音' : '静音'"
              @click="emit('mute')"
            >
              <SolarIcon
                :name="volumeIcon"
                :size="18"
              />
            </button>
          </template>
          <div class="volume-panel">
            <el-slider
              v-model="volumeValue"
              vertical
              height="104px"
              :show-tooltip="false"
              :disabled="disabled"
              aria-label="音量"
            />
            <span class="volume-num">{{ volumeValue }}%</span>
          </div>
        </el-popover>

        <!-- 小窗（独立播放器窗口内隐藏） -->
        <el-tooltip
          v-if="allowMini && !isRightCollapsed('mini')"
          :content="mini ? '退出小窗' : '小窗播放'"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :class="{ 'is-active': mini }"
            :disabled="fullscreen"
            :aria-pressed="mini"
            aria-label="小窗播放"
            @click="emit('toggle-mini')"
          >
            <SolarIcon
              :name="mini ? 'quit-pip' : 'pip'"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 弹窗播放：切到独立播放器窗口（挖洞定位 / HDR 场景）；独立窗口内隐藏 -->
        <el-tooltip
          v-if="allowPopOut"
          content="弹窗播放"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :disabled="disabled"
            aria-label="弹窗播放"
            @click="emit('pop-out')"
          >
            <SolarIcon
              name="square-arrow-right-up"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 全屏 -->
        <el-tooltip
          :content="fullscreen ? '退出全屏' : '全屏'"
          placement="top"
          :show-after="400"
        >
          <button
            type="button"
            class="pc-btn"
            :aria-label="fullscreen ? '退出全屏' : '全屏'"
            @click="emit('fullscreen')"
          >
            <SolarIcon
              :name="fullscreen ? 'quit-full-screen' : 'full-screen'"
              :size="18"
            />
          </button>
        </el-tooltip>

        <!-- 右侧按钮被收起时，更多按钮贴在本组最左侧 -->
        <el-popover
          v-if="rightCollapsed.size > 0"
          v-model:visible="moreRightOpen"
          placement="top"
          :width="200"
          :popper-options="{ modifiers: [{ name: 'preventOverflow', options: { padding: 8 } }] }"
          trigger="click"
          :popper-style="POPOVER_STYLE"
          :show-arrow="false"
          popper-class="player-popover more-right-popover"
        >
          <template #reference>
            <button
              type="button"
              class="pc-btn more-right-btn"
              :disabled="disabled"
              aria-label="更多操作"
              title="更多操作"
            >
              <SolarIcon name="menu-dots" :size="18" />
            </button>
          </template>
          <div class="pop-head">
            <span>更多操作</span>
          </div>
          <div class="more-menu">
            <button
              v-if="isRightCollapsed('danmaku')"
              type="button"
              class="track-item"
              :class="{ 'is-current': danmaku }"
              :disabled="disabled"
              @click="moreRightOpen = false; emit('toggle-danmaku')"
            >
              <SolarIcon name="chat-round-line" :size="14" />
              <span class="track-text">{{ danmaku ? '关闭弹幕' : '开启弹幕' }}</span>
            </button>
            <!-- 画质增强：二级浮层挂在一级菜单项上，菜单本身保持打开 -->
            <el-popover
              v-if="isRightCollapsed('enhance')"
              v-model:visible="moreEnhanceOpen"
              placement="left"
              :width="200"
              :popper-options="{ modifiers: [{ name: 'preventOverflow', options: { padding: 8, altAxis: true } }] }"
              trigger="click"
              :teleported="false"
              :popper-style="POPOVER_STYLE"
              :show-arrow="false"
              popper-class="player-popover"
            >
              <template #reference>
                <button
                  type="button"
                  class="track-item"
                  :class="{ 'is-current': currentEnhance !== 'off' }"
                  :disabled="disabled"
                >
                  <SolarIcon name="high-quality" :size="14" />
                  <span class="track-text">画质增强</span>
                  <span class="track-lang">{{ enhanceLabel }}</span>
                </button>
              </template>
              <div class="pop-head pop-head--plugins">
                <span>插件</span>
                <span class="pop-count">{{ props.pluginItems.filter((item) => item.enabled).length }} 个启用</span>
              </div>
              <div class="track-list track-list--plugins">
                <section
                  v-for="group in pluginGroups"
                  :key="group.key"
                  class="plugin-group"
                >
                  <div class="plugin-group-head">
                    <span>{{ group.title }}</span>
                    <span v-if="group.hint" class="plugin-group-hint">{{ group.hint }}</span>
                  </div>
                  <div v-for="plugin in group.items" :key="plugin.id" class="plugin-entry">
                    <button
                      type="button"
                      class="track-item"
                      :class="{ 'is-current': plugin.enabled }"
                      :title="plugin.error || plugin.description || plugin.name"
                      @click="onPluginClick(plugin)"
                    >
                      <SolarIcon
                        v-if="plugin.enabled"
                        name="check-circle"
                        :size="14"
                      />
                      <span class="track-text ellipsis-1">{{ plugin.name || plugin.id }}</span>
                      <span
                        v-if="plugin.videoFilters.length > 0"
                        class="plugin-decode is-soft"
                        title="启用后会切到 CPU 软解（lavfi 视频滤镜）"
                      >
                        软解
                      </span>
                      <span class="track-lang">{{ pluginSubLabel(plugin) }}</span>
                    </button>
                    <div v-if="presetOpenId === plugin.id" class="plugin-presets">
                      <button
                        v-for="shader in plugin.shaders"
                        :key="shader.id"
                        type="button"
                        class="track-item plugin-preset"
                        :class="{ 'is-current': plugin.enabled && plugin.preset === shader.id }"
                        :title="shader.gpuLoad ? `GPU 负载：${shader.gpuLoad}` : ''"
                        @click="onPickPreset(plugin, shader.id)"
                      >
                        <SolarIcon
                          v-if="plugin.enabled && plugin.preset === shader.id"
                          name="check-circle"
                          :size="14"
                        />
                        <span class="track-text ellipsis-1">{{ shader.name || shader.id }}</span>
                        <span v-if="gpuLoadLabel(shader.gpuLoad)" class="track-lang">
                          {{ gpuLoadLabel(shader.gpuLoad) }}
                        </span>
                      </button>
                    </div>
                  </div>
                </section>
              </div>
            </el-popover>

            <!-- 播放倍速：同样作为二级浮层 -->
            <el-popover
              v-if="isRightCollapsed('rate')"
              v-model:visible="moreRateOpen"
              placement="left-start"
              :width="140"
              trigger="click"
              :teleported="false"
              :popper-style="POPOVER_STYLE"
              :show-arrow="false"
              popper-class="player-popover"
            >
              <template #reference>
                <button
                  type="button"
                  class="track-item"
                  :class="{ 'is-current': Math.abs(state.rate - 1) > 0.01 }"
                  :disabled="disabled"
                >
                  <SolarIcon name="playback-speed" :size="14" />
                  <span class="track-text">播放倍速</span>
                  <span class="track-lang">{{ Number(currentRate) }}x</span>
                </button>
              </template>
              <div class="pop-head">
                <span>播放倍速</span>
                <span class="pop-count">{{ Number(currentRate) }}x</span>
              </div>
              <div class="track-list">
                <button
                  v-for="rate in RATE_OPTIONS"
                  :key="rate"
                  type="button"
                  class="track-item"
                  :class="{ 'is-current': Math.abs(state.rate - rate) < 0.01 }"
                  @click="onSelectRate(rate)"
                >
                  <SolarIcon
                    v-if="Math.abs(state.rate - rate) < 0.01"
                    name="check-circle"
                    :size="14"
                  />
                  <span class="track-text ellipsis-1">{{ rate }}x</span>
                </button>
              </div>
            </el-popover>
            <button
              v-if="allowMini && isRightCollapsed('mini')"
              type="button"
              class="track-item"
              :class="{ 'is-current': mini }"
              :disabled="disabled || fullscreen"
              @click="moreRightOpen = false; emit('toggle-mini')"
            >
              <SolarIcon :name="mini ? 'quit-pip' : 'pip'" :size="14" />
              <span class="track-text">{{ mini ? '退出小窗' : '小窗播放' }}</span>
            </button>
          </div>
        </el-popover>
      </div>
    </div>
  </div>
</template>

<style scoped>
.player-controls {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  z-index: 3;
  display: flex;
  flex-direction: column;
  gap: 2px;
  box-sizing: border-box;
  padding: 30px var(--space-4) var(--space-2);
  color: rgba(255, 255, 255, 0.92);
  background: linear-gradient(to bottom,
      rgba(0, 0, 0, 0) 0%,
      rgba(0, 0, 0, 0.58) 42%,
      rgba(0, 0, 0, 0.84) 100%);
  opacity: 0;
  pointer-events: none;
  transform: translateY(6px);
  transition:
    opacity 0.22s ease,
    transform 0.22s ease;
}

.player-controls.is-visible {
  opacity: 1;
  pointer-events: auto;
  transform: translateY(0);
}

.player-controls.is-disabled {
  opacity: 0.55;
}

.row {
  display: flex;
  align-items: center;
  min-width: 0;
}

/* ---------------- 第一行：进度条 + 总时长 ---------------- */

.progress-row {
  gap: var(--space-3);
  padding-bottom: 2px;
}

.progress {
  position: relative;
  flex: 1;
  min-width: 0;
  height: 22px;
  cursor: pointer;
  touch-action: none;
}

.progress.is-disabled {
  cursor: default;
  opacity: 0.6;
}

.progress-track {
  position: absolute;
  top: 50%;
  right: 0;
  left: 0;
  height: 4px;
  border-radius: 10000px;
  background: rgba(255, 255, 255, 0.28);
  transform: translateY(-50%);
}

.progress-fill {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  border-radius: 10000px;
  background: var(--accent);
  /* 后端状态约 5Hz 推送：补间过渡，避免进度条/滑块一格一格跳 */
  transition: width 0.18s linear;
}

/* 拖动进度条时实时跟手，不做补间 */
.progress.is-dragging .progress-fill,
.progress.is-dragging .progress-thumb {
  transition: none;
}

/* 缓存进度：半透明白，叠在轨道背景上、播放进度下方；宽度变化带过渡（后端约 5Hz 更新） */
.progress-buffer {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  border-radius: 10000px;
  background: rgba(255, 255, 255, 0.45);
  transition: width 0.2s ease;
  pointer-events: none;
}

/* 滑块 = 当前播放时间（胶囊） */
.progress-thumb {
  position: absolute;
  top: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  height: 20px;
  padding: 0 8px;
  border-radius: 10000px;
  background: var(--accent);
  color: #fff;
  font-family: var(--font-mono);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  line-height: 1;
  white-space: nowrap;
  box-shadow: 0 1px 6px rgba(0, 0, 0, 0.35);
  transform: translate(-50%, -50%);
  transition: left 0.18s linear;
  pointer-events: none;
}

/* hover 进度条：在光标位置显示该处时间 */
.progress-tip {
  position: absolute;
  bottom: calc(100% - 2px);
  padding: 2px 8px;
  border-radius: 10000px;
  background: rgba(0, 0, 0, 0.78);
  color: rgba(255, 255, 255, 0.94);
  font-family: var(--font-mono);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  transform: translateX(-50%);
  pointer-events: none;
}

.time-total {
  flex-shrink: 0;
  min-width: 46px;
  font-family: var(--font-mono);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: rgba(255, 255, 255, 0.78);
  text-align: right;
}

/* ---------------- 第二行：按钮组 ---------------- */

.buttons-row {
  gap: var(--space-2);
}

.btn-group {
  display: flex;
  flex-shrink: 0;
  gap: 2px;
  align-items: center;
  min-width: 0;
}

.btn-spacer {
  flex: 1;
  min-width: var(--space-2);
}

/* 「更多」贴在各自区域的内侧：左组最右、右组最左 */
.more-left-btn {
  order: 10;
}

.more-right-btn {
  order: -1;
}

.pc-btn {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: inherit;
  font: inherit;
  line-height: 1;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.pc-btn:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.16);
}

.pc-btn:disabled {
  cursor: not-allowed;
  opacity: 0.4;
}

.pc-btn.is-active {
  color: var(--accent);
}

/* 窄屏收进「更多」菜单的按钮：保留 DOM，只隐藏，避免弹层锚点丢失 */
.pc-collapsed {
  display: none !important;
}

/* 倍速按钮：图标 + 当前倍速文字（比纯图标略宽） */
.pc-btn.rate-btn {
  width: auto;
  gap: 3px;
  padding: 0 6px;
  font-variant-numeric: tabular-nums;
}

.rate-text {
  font-size: 11.5px;
  line-height: 1;
}

/* 播放/暂停：稍大一点，视觉重心 */
.pc-btn-main {
  width: 36px;
  height: 36px;
  margin: 0 2px;
}

.pc-btn-main:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.22);
}

/* 保存本集：保存中图标旋转 */
.is-spinning {
  animation: pc-spin 1s linear infinite;
}

@keyframes pc-spin {
  to {
    transform: rotate(360deg);
  }
}

/* ---------------- 片头 / 片尾（图标 + 时间 + x） ---------------- */

.mark-group {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 0 2px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
}

.mark-group.is-disabled {
  opacity: 0.45;
  pointer-events: none;
}

.mark-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: var(--radius-sm);
  color: rgba(255, 255, 255, 0.88);
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.mark-group:hover .mark-icon {
  background: rgba(255, 255, 255, 0.16);
}

.mark-icon.is-active {
  color: var(--accent);
  background: var(--accent-weak);
}

.mark-time {
  font-family: var(--font-mono);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  color: rgba(255, 255, 255, 0.78);
  white-space: nowrap;
}

.mark-clear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: rgba(255, 255, 255, 0.6);
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.mark-clear:hover {
  background: rgba(255, 255, 255, 0.16);
  color: #fff;
}

.mark-group--outro {
  margin-left: var(--space-2);
}

/* ---------------- 弹层内容 ---------------- */

.pop-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 6px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--text);
}

.pop-count {
  font-size: 11px;
  font-weight: 400;
  color: var(--text-muted);
}

.pop-empty {
  margin: 4px 0 2px;
  font-size: 12px;
  color: var(--text-muted);
}

.ep-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.ep-item,
.track-item {
  display: flex;
  gap: 6px;
  align-items: center;
  width: 100%;
  padding: 6px 8px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-regular);
  font: inherit;
  font-size: 12.5px;
  text-align: left;
  cursor: pointer;
}

.ep-item:hover,
.track-item:hover {
  background: var(--fill-1);
}

.ep-item.is-current,
.track-item.is-current {
  color: var(--accent);
  font-weight: 600;
  background: var(--accent-weak);
}

.ep-item-text,
.track-text {
  flex: 1;
  min-width: 0;
}

.track-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.track-lang {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-faint);
}

/* 外挂字幕标记 */
.track-tag {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: 999px;
  background: var(--accent-weak);
  color: var(--accent);
  font-size: 10px;
  line-height: 1.5;
}

/* 弹层底部「加载本地字幕」入口（与列表项同构，加一条分隔线） */
.track-add {
  display: flex;
  gap: 6px;
  align-items: center;
  width: 100%;
  margin-top: 4px;
  padding: 7px 8px;
  border: none;
  border-top: 1px solid var(--border);
  border-radius: 0 0 var(--radius-sm) var(--radius-sm);
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12.5px;
  text-align: left;
  cursor: pointer;
}

.track-add:hover:not(:disabled) {
  color: var(--accent);
  background: var(--fill-1);
}

.track-add:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 第二个入口（在线搜索）不再重复画分隔线 */
.track-add+.track-add {
  margin-top: 0;
  border-top: none;
}

.pop-actions {
  display: flex;
  gap: var(--space-2);
  margin-top: 2px;
}

.pop-hint {
  margin: 8px 0 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-faint);
}

/* ---------------- 音量弹层（参考网易云：窄竖条 + 百分比） ---------------- */

.volume-panel {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  align-items: center;
  width: 100%;
  padding: var(--space-2);
  padding-bottom: 0;
}

.volume-panel :deep(.el-slider.is-vertical) {
  --el-slider-height: 6px;
  --el-slider-button-size: 12px;
  --el-slider-button-wrapper-size: 22px;
  /* wrapper 居中到 6px 导轨：(22 - 6) / 2 = 8 */
  --el-slider-button-wrapper-offset: -8px;
  height: 104px;
}

.volume-panel :deep(.el-slider__runway) {
  width: 6px;
  margin: 0 auto;
  background: rgba(255, 255, 255, 0.18);
}

.volume-panel :deep(.el-slider__bar) {
  width: 6px;
  background: var(--accent);
}

.volume-panel :deep(.el-slider__button) {
  width: 12px;
  height: 12px;
  border: 2px solid #fff;
  background: var(--accent);
  box-shadow: 0 1px 4px rgb(0 0 0 / 45%);
}

.volume-panel :deep(.el-slider__button-wrapper) {
  background: transparent;
}

.volume-num {
  font-family: var(--font-mono);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  color: rgba(255, 255, 255, 0.7);
  line-height: 1;
}

/* 小窗 = 铺满窗口的纯播放器视图，控制条保持常规尺寸（无需压缩） */

/* 窄窗格压缩间距，避免按钮换行溢出 */
@media (max-width: 720px) {
  .player-controls {
    padding-right: var(--space-2);
    padding-left: var(--space-2);
  }

  .buttons-row {
    gap: var(--space-1);
  }

  .pc-btn {
    width: 28px;
    height: 28px;
  }

  .pc-btn-main {
    width: 32px;
    height: 32px;
  }
}

@media (prefers-reduced-motion: reduce) {

  .player-controls,
  .progress-track,
  .progress-buffer,
  .progress-thumb {
    transition: none;
  }
}

/* ---------------- 弹层深色配色（内容在 popper 内，沿用 scoped 属性） ---------------- */

.more-menu {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.more-volume {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
}

.more-volume :deep(.el-slider) {
  flex: 1;
  min-width: 90px;
}

.more-volume-num {
  font-family: var(--font-mono);
  font-size: 11px;
  color: rgba(255, 255, 255, 0.7);
}

.pop-head {
  color: rgba(255, 255, 255, 0.94);
}

.pop-count,
.pop-empty,
.pop-hint,
.track-lang,
.volume-num {
  color: rgba(255, 255, 255, 0.6);
}

.ep-item,
.track-item {
  color: rgba(255, 255, 255, 0.86);
}

.ep-item:hover,
.track-item:hover {
  background: rgba(255, 255, 255, 0.12);
}

.ep-item.is-current,
.track-item.is-current {
  color: #fff;
  background: rgba(255, 255, 255, 0.18);
}

.track-tag {
  background: rgba(255, 255, 255, 0.16);
  color: rgba(255, 255, 255, 0.9);
}

.track-add {
  color: rgba(255, 255, 255, 0.7);
  border-top-color: rgba(255, 255, 255, 0.12);
}

.track-add:hover:not(:disabled) {
  color: #fff;
  background: rgba(255, 255, 255, 0.12);
}

/* 下载与保存弹层：动作行下方是本片「边下边播」策略开关 */
.autosave-row {
  display: flex;
  gap: 10px;
  align-items: center;
  justify-content: space-between;
  margin-top: 6px;
  padding: 9px 8px 3px;
  border-top: 1px solid rgba(255, 255, 255, 0.12);
}

.autosave-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.autosave-title {
  font-size: 12.5px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.88);
}

.autosave-desc {
  font-size: 11px;
  line-height: 1.5;
  color: rgba(255, 255, 255, 0.6);
}

.autosave-switch {
  flex-shrink: 0;
}

.volume-mute {
  color: rgba(255, 255, 255, 0.8);
}

.volume-mute:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.14);
}

.volume-panel :deep(.el-slider__runway) {
  background: rgba(255, 255, 255, 0.18);
}
</style>

<style>
/* 播放器弹层统一深色半透明（popper 由 Element Plus teleport 到 body，不能用 scoped） */
.el-popper.el-popover.player-popover {
  min-width: 0;
  padding: var(--space-2);
  border: 1px solid rgba(255, 255, 255, 0.14);
  border-radius: var(--radius-md);
  background: rgba(18, 18, 22, 0.72);
  color: rgba(255, 255, 255, 0.9);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(14px) saturate(140%);
  -webkit-backdrop-filter: blur(14px) saturate(140%);
}

.el-popper.el-popover.player-popover .el-popper__arrow {
  display: none;
}

.el-popper.el-popover.player-popover .el-popper__arrow::before {
  border-color: rgba(255, 255, 255, 0.14);
  background: rgba(18, 18, 22, 0.72);
}

/* 弹层里的默认按钮也用半透明深色，避免白块 */
.el-popper.el-popover.player-popover .el-button:not(.el-button--primary) {
  --el-button-bg-color: rgba(255, 255, 255, 0.1);
  --el-button-border-color: rgba(255, 255, 255, 0.18);
  --el-button-text-color: rgba(255, 255, 255, 0.9);
  --el-button-hover-bg-color: rgba(255, 255, 255, 0.18);
  --el-button-hover-border-color: rgba(255, 255, 255, 0.28);
  --el-button-hover-text-color: #fff;
}

/* 画质浮层里的插件快捷列表：插件多时可滚动，避免浮层高过屏幕 */
.pop-head--plugins {
  margin-top: 8px;
}

.track-list--plugins {
  max-height: 220px;
  overflow: auto;
}

.plugin-group + .plugin-group {
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid rgba(255, 255, 255, 0.14);
}

.plugin-group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 8px;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.92);
}

.plugin-group-hint {
  opacity: 0.65;
}

.plugin-decode {
  flex: none;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 10px;
  line-height: 14px;
  font-weight: 500;
}

.plugin-decode.is-soft {
  color: #1b1b1f;
  background: rgba(230, 162, 60, 0.95);
}

.plugin-entry {
  display: flex;
  flex-direction: column;
}

.plugin-presets {
  display: flex;
  flex-direction: column;
  padding: 0px 0 0px 18px;
}
</style>
