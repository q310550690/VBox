<!-- 下载任务行（frontend-design.md §6.7）：按状态展示操作，状态机由 useDownloadsStore 负责 -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { DownloadAction, DownloadStatus, DownloadTask } from '@/types'
import { formatBytes, formatDuration, formatSpeed } from '@/utils/format'
import { explainMediaError } from '@/utils/mediaError'
import { taskDownloadedBytes, taskPercent, taskTotalBytes } from './downloadGroups'

const props = defineProps<{
  /** 单条下载任务（useDownloadsStore.tasks 的成员） */
  task: DownloadTask
  /** 紧凑模式：用于「一部剧一个卡片」的展开列表（隐藏封面、标题只显示集名） */
  compact?: boolean
}>()

const emit = defineEmits<{
  (e: 'control', id: string, action: DownloadAction): void
  (e: 'remove', task: DownloadTask): void
  (e: 'play', task: DownloadTask): void
  (e: 'open-location', task: DownloadTask): void
}>()

/** 已完成任务点击整行进入播放（无 filePath 时由下载页提示文件不存在） */
function onRowClick() {
  if (props.task.status === 'completed') emit('play', props.task)
}

/** 状态中文名 */
const STATUS_TEXT: Record<DownloadStatus, string> = {
  queued: '排队中',
  downloading: '下载中',
  paused: '已暂停',
  completed: '已完成',
  failed: '失败',
}

/** 无封面时用片名首字占位 */
const initial = computed(() => props.task.name.trim().charAt(0) || '影')

/** 已知总字节数（HLS 下载中为 0 = 未知，不能按字节算百分比） */
const totalBytes = computed(() => taskTotalBytes(props.task))

/** 已下载字节数（含总大小未知的 HLS） */
const downloadedBytes = computed(() => taskDownloadedBytes(props.task))

/** 进度百分比（0-100）：单集进度以后端 progress 为准 */
const percent = computed(() => Math.max(0, Math.min(100, Math.round(taskPercent(props.task)))))

/** 进度条用未取整值 + CSS 过渡，和后端约 5Hz 的进度推送对齐 */
const percentExact = computed(() => Math.max(0, Math.min(100, taskPercent(props.task))))

/** 总大小未知的进行中任务：显示不定进度，不显示假的百分比 */
const indeterminate = computed(
  () =>
    props.task.status !== 'completed' &&
    totalBytes.value <= 0 &&
    (props.task.status === 'downloading' || props.task.status === 'paused'),
)

/** 百分比文案 */
const percentText = computed(() => {
  if (props.task.status === 'completed') return '100%'
  if (indeterminate.value) return '—'
  return `${percent.value}%`
})

const statusText = computed(() => STATUS_TEXT[props.task.status])

/** 紧凑行显示在集名后的文件大小（未知时为空） */
const sizeText = computed(() => (totalBytes.value > 0 ? formatBytes(totalBytes.value) : ''))

/** 已下载大小文案（未知总大小时尤其重要：26.0 MB） */
const downloadedText = computed(() =>
  downloadedBytes.value > 0 ? formatBytes(downloadedBytes.value) : '',
)

/** 实时速度文案：下载中显示 KB/s / MB/s；暂停显示 0 B/s；其余为空 */
const speedText = computed(() => {
  const { status, speedBps } = props.task
  if (status === 'downloading') {
    return speedBps && speedBps > 0 ? `↓ ${formatSpeed(speedBps)}` : '↓ 0 B/s'
  }
  if (status === 'paused') return '↓ 0 B/s'
  return ''
})

/** 剩余时间（仅下载中且已知速率/体积时计算，不编造） */
const remainingText = computed(() => {
  const { speedBps } = props.task
  if (!speedBps || speedBps <= 0 || totalBytes.value <= 0) return ''
  const remainBytes = ((100 - percent.value) / 100) * totalBytes.value
  return formatDuration(Math.ceil(remainBytes / speedBps))
})

/** 行内次要信息：按状态给出下载速度 / 体积 / 失败原因等 */
const metaText = computed(() => {
  const task = props.task
  const size = sizeText.value
  const downloaded = downloadedText.value
  switch (task.status) {
    case 'downloading': {
      const parts = [speedText.value]
      if (downloaded) parts.push(`已下载 ${downloaded}`)
      if (remainingText.value) parts.push(`剩余 ${remainingText.value}`)
      if (size) parts.push(`共 ${size}`)
      return parts.join(' · ')
    }
    case 'paused':
      return downloaded ? `已暂停 · 已下载 ${downloaded}` : '已暂停'
    case 'queued':
      return size ? `等待开始 · 共 ${size}` : '等待开始'
    case 'completed':
      if (!task.filePath) {
        return size ? `已完成 · ${size} · 文件路径未知` : '已完成 · 文件路径未知'
      }
      return size ? `已完成 · ${size} · 点击进入播放` : '已完成 · 点击进入播放'
    case 'failed':
      return downloaded ? `已下载 ${downloaded} · 共 ${size || '未知'}` : '下载失败'
    default:
      return size
  }
})
</script>

<template>
  <article
    class="download-row"
    :class="[`is-${task.status}`, { 'is-compact': compact }]"
    :title="task.status === 'completed' ? '点击进入播放' : undefined"
    @click="onRowClick"
  >
    <div v-if="!compact" class="cover">
      <img v-if="task.cover" class="cover-img" :src="task.cover" :alt="task.name" loading="lazy" />
      <span v-else class="cover-fallback">{{ initial }}</span>
    </div>

    <div class="info">
      <div class="row-top">
        <h3 class="name ellipsis-1">
          {{ compact ? task.epTitle : `${task.name} · ${task.epTitle}` }}
          <span v-if="compact && sizeText" class="name-size">· {{ sizeText }}</span>
          <!-- 紧凑行也要能看到实时速度（暂停时显示 0 B/s） -->
          <span
            v-if="compact && speedText"
            class="name-speed"
            :class="`speed-${task.status}`"
            :title="task.status === 'downloading' ? '当前下载速度' : undefined"
          >
            · {{ speedText }}
          </span>
        </h3>
        <span class="status" :class="`status-${task.status}`">
          {{ statusText }}
        </span>
      </div>

      <p v-if="!compact" class="meta ellipsis-1">{{ metaText }}</p>

      <!-- 总大小未知时进度是「不定进度」：显示条纹动画 + 「—」，避免出现假的 100% -->
      <div class="bar-line">
        <div class="bar" aria-hidden="true">
          <span
            class="bar-fill"
            :class="{ 'is-indeterminate': indeterminate }"
            :style="{ width: `${percentExact}%` }"
          />
        </div>
        <span class="bar-percent">{{ percentText }}</span>
      </div>

      <p v-if="task.error" class="error ellipsis-1" :title="task.error">{{ explainMediaError(task.error, 'download') }}</p>

      <p
        v-if="!compact && task.status === 'completed' && task.filePath"
        class="file-path ellipsis-1"
        :title="task.filePath"
      >
        {{ task.filePath }}
      </p>

      <!-- 操作区阻止冒泡：点按钮不应同时触发整行「进入播放」；统一圆形图标按钮 -->
      <div class="actions" @click.stop>
        <template v-if="task.status === 'downloading'">
          <el-tooltip content="暂停" placement="top">
            <el-button circle aria-label="暂停" @click="emit('control', task.id, 'pause')">
              <template #icon><SolarIcon name="pause" :size="16" /></template>
            </el-button>
          </el-tooltip>
          <el-tooltip content="取消" placement="top">
            <el-button circle aria-label="取消" @click="emit('control', task.id, 'cancel')">
              <template #icon><SolarIcon name="close" :size="16" /></template>
            </el-button>
          </el-tooltip>
        </template>
        <template v-else-if="task.status === 'paused'">
          <el-tooltip content="继续" placement="top">
            <el-button circle aria-label="继续" @click="emit('control', task.id, 'resume')">
              <template #icon><SolarIcon name="play" :size="16" /></template>
            </el-button>
          </el-tooltip>
          <el-tooltip content="取消" placement="top">
            <el-button circle aria-label="取消" @click="emit('control', task.id, 'cancel')">
              <template #icon><SolarIcon name="close" :size="16" /></template>
            </el-button>
          </el-tooltip>
        </template>
        <template v-else-if="task.status === 'queued'">
          <el-tooltip content="取消" placement="top">
            <el-button circle aria-label="取消" @click="emit('control', task.id, 'cancel')">
              <template #icon><SolarIcon name="close" :size="16" /></template>
            </el-button>
          </el-tooltip>
        </template>
        <template v-else-if="task.status === 'failed'">
          <el-tooltip content="重试" placement="top">
            <el-button
              circle
              type="primary"
              aria-label="重试"
              @click="emit('control', task.id, 'retry')"
            >
              <template #icon><SolarIcon name="refresh" :size="16" /></template>
            </el-button>
          </el-tooltip>
          <el-tooltip content="删除记录" placement="top">
            <el-button
              circle
              type="danger"
              plain
              aria-label="删除记录"
              @click="emit('remove', task)"
            >
              <template #icon><SolarIcon name="trash-bin-minimalistic" :size="16" /></template>
            </el-button>
          </el-tooltip>
        </template>
        <template v-else>
          <el-tooltip content="播放" placement="top">
            <el-button circle type="primary" aria-label="播放" @click="emit('play', task)">
              <template #icon><SolarIcon name="play" :size="16" /></template>
            </el-button>
          </el-tooltip>
          <el-tooltip content="打开所在位置" placement="top">
            <el-button circle aria-label="打开所在位置" @click="emit('open-location', task)">
              <template #icon><SolarIcon name="folder-open" :size="16" /></template>
            </el-button>
          </el-tooltip>
          <el-tooltip content="删除记录" placement="top">
            <el-button
              circle
              type="danger"
              plain
              aria-label="删除记录"
              @click="emit('remove', task)"
            >
              <template #icon><SolarIcon name="trash-bin-minimalistic" :size="16" /></template>
            </el-button>
          </el-tooltip>
        </template>
      </div>
    </div>
  </article>
</template>

<style scoped>
.download-row {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: var(--space-2);
  transition: background-color 0.18s ease;
}

.download-row:hover {
  background: var(--surface-2);
}

.download-row.is-completed {
  cursor: pointer;
}

/* 剧集卡片内的紧凑行：左侧信息 + 右侧操作，两栏之间一根竖分割线 */
.download-row.is-compact {
  align-items: center;
  padding: var(--space-3);
  gap: var(--space-2);
}

.download-row.is-compact .info {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  grid-template-areas:
    "top actions"
    "bar actions";
  align-items: center;
  column-gap: var(--space-2);
  row-gap: 2px;
}

.download-row.is-compact .row-top {
  grid-area: top;
}

.download-row.is-compact .bar-line {
  grid-area: bar;
  margin-top: 0;
}

/* 操作列：右对齐 + 左侧竖分割线（贯穿标题/信息/进度条三行） */
.download-row.is-compact .actions {
  grid-area: actions;
  align-self: stretch;
  justify-content: flex-end;
  align-items: center;
  margin-top: 0;
  padding-left: var(--space-2);
  border-left: 1px solid var(--border-lighter);
}

.download-row.is-compact .error,
.download-row.is-compact .file-path {
  grid-column: 1 / -1;
}

.download-row.is-compact .name {
  font-size: 13px;
  font-weight: 500;
}

.file-path {
  margin: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
}

.cover {
  flex: 0 0 56px;
  width: 56px;
  height: 80px;
  border-radius: var(--radius-md);
  overflow: hidden;
  background: var(--surface-2);
}

.cover-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.cover-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: var(--text-muted);
  font-size: 20px;
  font-weight: 600;
}

.info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.row-top {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-3);
}

.name {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  min-width: 0;
}

/* 紧凑行：文件大小直接跟在集名后面 */
.name-size {
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}

/* 紧凑行的实时速度（下载中 / 暂停 0 B/s） */
.name-speed {
  color: var(--accent);
  font-size: 11.5px;
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.name-speed.speed-paused {
  color: var(--warning);
}

.status {
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

.meta {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

/* 与父级 series-progress-row 一致：进度条 + 紧跟其后的百分比 */
.bar-line {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  margin-top: var(--space-1);
}

.bar {
  flex: 1;
  min-width: 0;
  height: 4px;
  border-radius: var(--radius-pill);
  background: var(--surface-3);
  overflow: hidden;
}

.bar-percent {
  flex-shrink: 0;
  min-width: 34px;
  color: var(--accent);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}

.is-completed .bar-percent {
  color: var(--success);
}

.is-failed .bar-percent {
  color: var(--danger);
}

.is-paused .bar-percent {
  color: var(--warning);
}

.bar-fill {
  display: block;
  height: 100%;
  border-radius: var(--radius-pill);
  background: var(--accent);
  transition: width 0.2s ease;
}

/* 总大小未知：进度是估算/不定进度，用流动高光提示 */
.bar-fill.is-indeterminate {
  background-image: linear-gradient(
    90deg,
    transparent 0%,
    rgba(255, 255, 255, 0.55) 50%,
    transparent 100%
  );
  background-repeat: no-repeat;
  background-size: 45% 100%;
  animation: bar-shimmer 1.3s linear infinite;
}

@keyframes bar-shimmer {
  from {
    background-position: -60% 0;
  }

  to {
    background-position: 160% 0;
  }
}

.is-completed .bar-fill {
  background: var(--success);
}

.is-failed .bar-fill {
  background: var(--danger);
}

.is-paused .bar-fill {
  background: var(--warning);
}

.error {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
}

.actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-1);
  flex-wrap: wrap;
}

@media (max-width: 720px) {
  .actions {
    justify-content: flex-start;
  }

  /* 窄窗口：紧凑行保持两列，按钮右对齐；信息过长时省略 */
  .download-row.is-compact .actions {
    justify-content: flex-end;
    margin-top: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .download-row,
  .bar-fill {
    transition: none;
  }

  .bar-fill.is-indeterminate {
    animation: none;
  }
}
</style>
