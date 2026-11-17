<!-- 下载页 · 剧集卡片：一部剧一张卡片，头部显示整组总进度/状态，展开后显示每集进度（可折叠） -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import { formatBytes } from '@/utils/format'
import DownloadRow from './DownloadRow.vue'
import {
  groupSpeedText,
  groupStatusText,
  type DownloadSeriesGroup,
} from './downloadGroups'
import type { DownloadAction, DownloadTask } from '@/types'

const props = defineProps<{
  /** 一部剧的聚合数据（buildDownloadGroups 的输出） */
  group: DownloadSeriesGroup
  /** 是否展开（展开后显示每集下载行） */
  expanded: boolean
}>()

const emit = defineEmits<{
  (e: 'toggle', key: string): void
  (e: 'control', id: string, action: DownloadAction): void
  (e: 'remove', task: DownloadTask): void
  (e: 'play', task: DownloadTask): void
  (e: 'open-location', task: DownloadTask): void
  (e: 'bulk', action: 'download' | 'pause', ids: string[]): void
}>()

/** 本剧「可暂停」的任务（下载中/排队中） */
const pauseIds = computed(() =>
  props.group.tasks
    .filter((task) => task.status === 'downloading' || task.status === 'queued')
    .map((task) => task.id),
)

/** 本剧「可下载」的任务（已暂停 / 失败） */
const downloadIds = computed(() =>
  props.group.tasks
    .filter((task) => task.status === 'paused' || task.status === 'failed')
    .map((task) => task.id),
)

const canPause = computed(() => pauseIds.value.length > 0)
const canDownload = computed(() => downloadIds.value.length > 0)

/* 卡片头部总开关：整组下载（继续/重试）或整组暂停 */
function onBulkDownload() {
  emit('bulk', 'download', downloadIds.value)
}

function onBulkPause() {
  emit('bulk', 'pause', pauseIds.value)
}

/** 总进度百分比（0-100 整数，用于文案） */
const percent = computed(() => Math.max(0, Math.min(100, Math.round(props.group.progress))))

/** 进度条用未取整值 + CSS 过渡，避免后端 5Hz 更新时一跳一跳 */
const percentExact = computed(() => Math.max(0, Math.min(100, props.group.progress)))

/** 百分比文案：总大小未知时是估算值（加 ~ 前缀），避免看起来「精确 100%」 */
const percentText = computed(() => {
  if (props.group.status === 'completed') return '100%'
  return props.group.indeterminate ? `~${percent.value}%` : `${percent.value}%`
})

/** 组状态文案 */
const statusText = computed(() => groupStatusText(props.group))

/** 速度文案（下载中显示实时速度，暂停显示「已暂停」） */
const speedText = computed(() => groupSpeedText(props.group))

/** 封面缺失时的首字占位 */
const initial = computed(() => (props.group.name || '影').trim().charAt(0))

/** 头部次要信息：站点 · 线路 · 集数 · 已完成/失败 · 总大小 · 已下载（未知大小时） */
const metaText = computed(() => {
  const g = props.group
  const parts: string[] = []
  if (g.siteName) parts.push(g.siteName)
  if (g.line) parts.push(`线路 ${g.line}`)
  parts.push(`共 ${g.total} 集`)
  if (g.completed > 0) parts.push(`已完成 ${g.completed}/${g.total}`)
  if (g.failed > 0) parts.push(`失败 ${g.failed}`)
  if (g.sizeBytes > 0) parts.push(`共 ${formatBytes(g.sizeBytes)}`)
  // 总大小未知（HLS 下载中）时给出已下载大小，进度按每集估算
  if (g.indeterminate && g.downloadedBytes > 0) {
    parts.push(`已下载 ${formatBytes(g.downloadedBytes)}`)
  }
  return parts.join(' · ')
})

/** 卡片状态 class（沿用单集状态色） */
const statusClass = computed(() => `is-${props.group.status}`)

function onToggle() {
  emit('toggle', props.group.key)
}

function onHeadKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    onToggle()
  }
}

/* 展开列表里的单集操作：原样向上抛给下载页（由页面统一调 store / 提示） */
function onControl(id: string, action: DownloadAction) {
  emit('control', id, action)
}

function onRemove(task: DownloadTask) {
  emit('remove', task)
}

function onPlay(task: DownloadTask) {
  emit('play', task)
}

function onOpenLocation(task: DownloadTask) {
  emit('open-location', task)
}
</script>

<template>
  <article class="series-card" :class="statusClass">
    <!-- 头部：点击整行折叠/展开（键盘 Enter/Space 同样可切换） -->
    <header
      class="series-head"
      role="button"
      tabindex="0"
      :aria-expanded="expanded"
      :aria-label="`${group.name}，${percentText}，${expanded ? '点击收起' : '点击展开'}`"
      @click="onToggle"
      @keydown="onHeadKeydown"
    >
      <div class="series-cover">
        <img
          v-if="group.cover"
          class="series-cover-img"
          :src="group.cover"
          :alt="group.name"
          loading="lazy"
        />
        <span v-else class="series-cover-fallback" aria-hidden="true">{{ initial }}</span>
      </div>

      <div class="series-main">
        <div class="series-top">
          <h3 class="series-name ellipsis-1" :title="group.name">{{ group.name }}</h3>
          <span class="series-status" :class="`status-${group.status}`">{{ statusText }}</span>
        </div>

        <p class="series-meta ellipsis-1" :title="metaText">{{ metaText }}</p>

        <!-- 最外层总进度：未知总大小时用「~估算值 + 流动条纹」表示不定进度 -->
        <div class="series-progress-row">
          <span class="series-bar" aria-hidden="true">
            <span
              class="series-bar-fill"
              :class="{ 'is-indeterminate': group.indeterminate }"
              :style="{ width: `${percentExact}%` }"
            />
          </span>
          <span class="series-percent">{{ percentText }}</span>
          <span
            v-if="speedText"
            class="series-speed"
            :class="`speed-${group.status}`"
            :title="group.status === 'downloading' ? '当前下载速度' : undefined"
          >
            {{ speedText }}
          </span>
        </div>
      </div>

      <!-- 整组总开关：下载（继续/重试） / 暂停 -->
      <div class="series-actions" @click.stop @keydown.stop>
        <el-tooltip content="本剧全部下载（继续/重试）" placement="top">
          <el-button
            circle
            :disabled="!canDownload"
            :aria-label="`${group.name} 全部下载`"
            @click="onBulkDownload"
          >
            <template #icon><SolarIcon name="download" :size="16" /></template>
          </el-button>
        </el-tooltip>
        <el-tooltip content="本剧全部暂停" placement="top">
          <el-button
            circle
            :disabled="!canPause"
            :aria-label="`${group.name} 全部暂停`"
            @click="onBulkPause"
          >
            <template #icon><SolarIcon name="pause" :size="16" /></template>
          </el-button>
        </el-tooltip>
      </div>
    </header>

    <!-- 展开：每集一行（紧凑模式，保留单集操作与「点击播放」） -->
    <div v-if="expanded" class="series-body">
      <DownloadRow
        v-for="task in group.tasks"
        :key="task.id"
        :task="task"
        compact
        @control="onControl"
        @remove="onRemove"
        @play="onPlay"
        @open-location="onOpenLocation"
      />
    </div>

    <!-- 卡片底部：独立的展开 / 收起条 -->
    <footer class="series-foot">
      <button
        type="button"
        class="series-expand"
        :aria-label="expanded ? '收起剧集' : '展开剧集'"
        :aria-expanded="expanded"
        @click="onToggle"
      >
        <span class="expand-icon" :class="{ 'is-expanded': expanded }">
          <SolarIcon name="alt-arrow-down" :size="14" />
        </span>
        <span>{{ expanded ? '收起' : `展开全部 ${group.total} 集` }}</span>
      </button>
    </footer>
  </article>
</template>

<style scoped>
.series-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  overflow: hidden;
}

.series-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-3);
  cursor: pointer;
  transition: background-color 0.18s ease;
}

.series-head:hover {
  background: var(--surface-2);
}

.series-head:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}

.series-cover {
  flex: 0 0 48px;
  width: 48px;
  height: 68px;
  overflow: hidden;
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.series-cover-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.series-cover-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: var(--text-muted);
  font-size: 18px;
  font-weight: 600;
}

.series-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.series-top {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-3);
}

.series-name {
  margin: 0;
  min-width: 0;
  color: var(--text);
  font-size: 14.5px;
  font-weight: 600;
}

.series-status {
  flex-shrink: 0;
  font-size: 12px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.status-downloading {
  color: var(--accent);
}

.status-queued {
  color: var(--text-muted);
}

.status-paused {
  color: var(--warning);
}

.status-completed {
  color: var(--success);
}

.status-failed {
  color: var(--danger);
}

.series-meta {
  margin: 0;
  color: var(--text-muted);
  font-size: 12px;
}

.series-progress-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  margin-top: 2px;
}

.series-bar {
  flex: 1;
  height: 5px;
  overflow: hidden;
  border-radius: var(--radius-pill);
  background: var(--surface-3);
}

.series-bar-fill {
  display: block;
  height: 100%;
  border-radius: var(--radius-pill);
  background: var(--accent);
  transition: width 0.2s ease;
}

/* 未知总大小（HLS 下载中）：进度是估算值，用流动高光提示「不定进度」 */
.series-bar-fill.is-indeterminate {
  background-image: linear-gradient(
    90deg,
    transparent 0%,
    rgba(255, 255, 255, 0.55) 50%,
    transparent 100%
  );
  background-repeat: no-repeat;
  background-size: 45% 100%;
  animation: series-bar-shimmer 1.3s linear infinite;
}

@keyframes series-bar-shimmer {
  from {
    background-position: -60% 0;
  }

  to {
    background-position: 160% 0;
  }
}

.series-speed {
  flex-shrink: 0;
  color: var(--accent);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.series-speed.speed-paused {
  color: var(--warning);
}

.is-completed .series-bar-fill {
  background: var(--success);
}

.is-failed .series-bar-fill {
  background: var(--danger);
}

.is-paused .series-bar-fill {
  background: var(--warning);
}

.series-percent {
  flex-shrink: 0;
  min-width: 34px;
  color: var(--accent);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}

.is-completed .series-percent {
  color: var(--success);
}

.is-failed .series-percent {
  color: var(--danger);
}

.is-paused .series-percent {
  color: var(--warning);
}

.series-actions {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  align-self: stretch;
  gap: var(--space-2);
  padding-left: var(--space-2);
  border-left: 1px solid var(--border-lighter);
}

/* 卡片底部：独立的整行展开 / 收起条 */
.series-foot {
  border-top: 1px solid var(--border-lighter);
}

.series-expand {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: center;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2);
  border: none;
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.series-expand:hover {
  background: var(--surface-2);
  color: var(--text);
}

.expand-icon {
  display: inline-flex;
  transition: transform 0.18s ease;
}

.expand-icon.is-expanded {
  transform: rotate(180deg);
}

.series-body {
  border-top: 1px solid var(--border-lighter);
}

.series-body :deep(.download-row + .download-row) {
  border-top: 1px solid var(--border-lighter);
}

@media (prefers-reduced-motion: reduce) {
  .series-head,
  .series-bar-fill,
  .expand-icon {
    transition: none;
  }

  .series-bar-fill.is-indeterminate {
    animation: none;
  }
}
</style>
