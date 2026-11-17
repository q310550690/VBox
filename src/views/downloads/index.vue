<!-- 下载页（frontend-design.md §6.7）：一部剧一张卡片，头部显示整组总进度/状态，可折叠展开每集进度
     数据来自 Rust 下载引擎（浏览器预览走 mock + localStorage，1s ticker 推进演示状态）。 -->
<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import * as api from '@/api'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import HintBar from '@/components/HintBar.vue'
import StatusTabs from '@/components/StatusTabs.vue'
import DownloadGroup from './components/DownloadGroup.vue'
import { buildDownloadGroups } from './components/downloadGroups'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import { useDownloadsStore } from '@/store/downloads'
import { useUiStore } from '@/store/ui'
import { formatSpeed } from '@/utils/format'
import type { DownloadAction, DownloadTask } from '@/types'

defineOptions({ name: 'DownloadsView' })

const router = useRouter()
const ui = useUiStore()
const store = useDownloadsStore()

/** 浏览器预览模式：下载任务为本地演示数据 */
const isDemo = api.demoMode

/** 删除已结束任务（completed / failed）前的二次确认 */
const confirmOpen = ref(false)
const pendingDelete = ref<DownloadTask | null>(null)

/** 状态筛选：全部 / 已完成 / 下载中 / 暂停中 */
type StatusTab = 'all' | 'completed' | 'downloading' | 'paused'

const TABS: { value: StatusTab; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'completed', label: '已完成' },
  { value: 'downloading', label: '下载中' },
  { value: 'paused', label: '暂停中' },
]

const activeTab = ref<StatusTab>('all')

/** 先按状态过滤任务，再聚合成剧集卡片（卡片统计随筛选变化） */
const filteredTasks = computed<DownloadTask[]>(() => {
  switch (activeTab.value) {
    case 'completed':
      return store.tasks.filter((task) => task.status === 'completed')
    case 'downloading':
      return store.tasks.filter(
        (task) => task.status === 'downloading' || task.status === 'queued',
      )
    case 'paused':
      return store.tasks.filter((task) => task.status === 'paused')
    default:
      return store.tasks
  }
})

const groups = computed(() => buildDownloadGroups(filteredTasks.value))

/** 全部下载任务的实时总速度（页头展示；暂停任务不贡献速度） */
const totalSpeedText = computed(() =>
  store.totalSpeedBps > 0 ? `↓ ${formatSpeed(store.totalSpeedBps)}` : '',
)

/** 卡片展开状态：首次出现的组有进行中任务时默认展开，其余默认收起；用户点过后保留选择 */
const expanded = ref<Record<string, boolean>>({})

watch(
  groups,
  (list) => {
    for (const group of list) {
      if (expanded.value[group.key] === undefined) expanded.value[group.key] = group.active > 0
    }
  },
  { immediate: true },
)

function onToggleGroup(key: string) {
  expanded.value = { ...expanded.value, [key]: !expanded.value[key] }
}

/** 删除时是否同时删除本地文件（默认关闭，防止误删） */
const removeFile = ref(false)

const confirmMessage = computed(() => {
  const task = pendingDelete.value
  if (!task) return ''
  return `确认删除「${task.name} · ${task.epTitle}」的下载记录？勾选下方选项可一起删除本地文件。`
})

/** 「全部暂停」仅在存在可暂停任务时可用 */
const canPauseAll = computed(() =>
  store.tasks.some((task) => task.status === 'downloading' || task.status === 'queued'),
)

/** 「全部下载」：有暂停或失败任务时可用（继续/重试） */
const canDownloadAll = computed(() =>
  store.tasks.some((task) => task.status === 'paused' || task.status === 'failed'),
)

/** 当前筛选下是否一个任务都没有 */
const filterEmpty = computed(() => !store.isEmpty && groups.value.length === 0)

/** 打开当前下载目录（demo 无本地目录，仅提示） */
function onLocalFileTips() {
  ui.notify('把视频或音频文件拖到窗口任意位置即可直接播放', 'info')
}

async function onOpenDir() {
  if (isDemo) {
    ui.notify('浏览器预览模式不支持打开本地目录', 'info')
    return
  }
  try {
    await api.downloadOpenDir()
    ui.notify('已打开下载目录', 'info')
  } catch (e) {
    ui.notify('打开目录失败：' + api.formatError(e), 'error')
  }
}

/** 打开所在位置：定位到具体文件（资源管理器/Finder 选中）；无本地文件时提示 */
async function onOpenLocation(task: DownloadTask) {
  if (isDemo) {
    ui.notify('浏览器预览模式不支持打开本地文件', 'info')
    return
  }
  if (!task.filePath) {
    ui.notify('该任务还没有本地文件', 'warning')
    return
  }
  try {
    await api.downloadOpenFileLocation(task.id)
  } catch (e) {
    ui.notify('打开所在位置失败：' + api.formatError(e), 'error')
  }
}

/** 进行中任务的操作：暂停 / 继续 / 重试 / 取消（取消直接移除，无需确认） */
async function onControl(id: string, action: DownloadAction) {
  const ok = await store.control(id, action)
  if (!ok) {
    ui.notify('操作失败：' + (store.error || '未知错误'), 'error')
    return
  }
  // 暂停/继续后状态会切到另一个 tab：跟着切一下，否则卡片会「凭空消失」，
  // 看起来像「点了继续但下载页没反应」。
  if (action === 'pause' && activeTab.value === 'downloading') activeTab.value = 'paused'
  if (action === 'resume' && activeTab.value === 'paused') activeTab.value = 'downloading'
  switch (action) {
    case 'pause':
      ui.notify('已暂停该下载任务', 'info')
      break
    case 'resume':
      ui.notify('已继续该下载任务', 'info')
      break
    case 'retry':
      ui.notify('已重新加入下载队列', 'info')
      break
    case 'cancel':
      ui.notify('已取消该下载任务', 'info')
      break
    default:
      break
  }
}

/** 删除：已结束任务走二次确认（可选同时删除本地文件） */
function onRemove(task: DownloadTask) {
  pendingDelete.value = task
  removeFile.value = false
  confirmOpen.value = true
}

async function onConfirmRemove() {
  const task = pendingDelete.value
  const withFile = removeFile.value
  if (task) {
    const ok = await store.remove(task.id, withFile)
    if (ok) {
      ui.notify(
        withFile ? '已删除下载记录和本地文件' : '已删除下载记录（本地文件保留）',
        'success',
      )
    } else {
      ui.notify('删除失败：' + (store.error || '未知错误'), 'error')
    }
  }
  pendingDelete.value = null
  removeFile.value = false
  confirmOpen.value = false
}

/** 播放已完成任务：有本地文件路径才跳转本地播放页 */
function onPlay(task: DownloadTask) {
  if (!task.filePath) {
    ui.notify('文件路径未知或文件不存在，无法播放', 'warning')
    return
  }
  void router.push({
    path: `/local/${encodeURIComponent(task.id)}`,
    query: { path: task.filePath, title: `${task.name} · ${task.epTitle}` },
  })
}

async function onPauseAll() {
  const ok = await store.pauseAll()
  if (ok) ui.notify('已暂停全部下载任务', 'info')
  else ui.notify('暂停失败：' + (store.error || '未知错误'), 'error')
}

/** 全部下载：继续所有已暂停 + 重试所有失败 */
async function onDownloadAll() {
  const ok = await store.downloadAll()
  if (ok) ui.notify('已继续/重试全部可下载任务', 'info')
  else ui.notify('操作失败：' + (store.error || '未知错误'), 'error')
}

/** 剧集卡片的总开关：整组下载（继续/重试）或整组暂停 */
async function onGroupBulk(action: 'download' | 'pause', ids: string[]) {
  if (!ids.length) return
  const ok =
    action === 'pause' ? await store.controlMany(ids, 'pause') : await store.downloadMany(ids)
  if (ok) {
    if (action === 'pause' && activeTab.value === 'downloading') activeTab.value = 'paused'
    if (action === 'download' && activeTab.value === 'paused') activeTab.value = 'downloading'
    ui.notify(action === 'pause' ? '已暂停该剧集下载' : '已继续/重试该剧集下载', 'info')
  } else {
    ui.notify('操作失败：' + (store.error || '未知错误'), 'error')
  }
}

function goVod() {
  void router.push('/vod')
}

onMounted(() => {
  // store.load() 内部会启动 1s 演示进度 ticker（演示状态机）
  void store.load()
})
</script>

<template>
  <PageScroll>
    <div class="page downloads-page">
    <!-- 页头：标题 + 状态筛选 tab + 统计/目录 + 全局圆形操作 -->
    <header class="page-header">
      <div class="header-left">
        <StatusTabs v-model="activeTab" :items="TABS" aria-label="下载状态筛选" />
      </div>
      <div class="toolbar">
        <span class="stats">
          共 {{ groups.length }} 部 · 下载中 {{ store.counts.downloading }} · 已完成
          {{ store.counts.completed }} · 失败 {{ store.counts.failed }}
          <template v-if="totalSpeedText"> · <span class="stats-speed">{{ totalSpeedText }}</span></template>
        </span>
        <span class="dir" :title="store.settings.dir">
          目录：{{ store.settings.dir || '加载中…' }}
        </span>
        <el-tooltip content="播放本地文件：把视频/音频拖到窗口任意位置" placement="bottom">
          <el-button circle aria-label="播放本地文件" @click="onLocalFileTips">
            <template #icon><SolarIcon name="play" :size="16" /></template>
          </el-button>
        </el-tooltip>
        <el-tooltip content="全部下载（继续暂停 / 重试失败）" placement="bottom">
          <el-button
            circle
            :disabled="!canDownloadAll"
            aria-label="全部下载"
            @click="onDownloadAll"
          >
            <template #icon><SolarIcon name="download" :size="16" /></template>
          </el-button>
        </el-tooltip>
        <el-tooltip content="全部暂停" placement="bottom">
          <el-button
            circle
            :disabled="!canPauseAll"
            aria-label="全部暂停"
            @click="onPauseAll"
          >
            <template #icon><SolarIcon name="pause" :size="16" /></template>
          </el-button>
        </el-tooltip>
        <el-tooltip content="打开下载目录" placement="bottom">
          <el-button circle aria-label="打开目录" @click="onOpenDir">
            <template #icon><SolarIcon name="folder-open" :size="16" /></template>
          </el-button>
        </el-tooltip>
      </div>
    </header>

    <!-- 浏览器预览才提示：任务为本地演示数据，不会真实下载 -->
    <HintBar v-if="isDemo">
      浏览器预览模式：以下为本地演示数据，不会真实下载；桌面端由 Rust 下载引擎执行。
    </HintBar>

    <SkeletonGrid v-if="store.loading && !store.initialized" variant="row" :count="4" />

    <ErrorState
      v-else-if="store.error && !store.tasks.length"
      :message="store.error"
      @retry="store.load"
    />

    <EmptyState
      v-else-if="store.isEmpty"
      title="暂无下载任务"
      desc="在详情页点「下载」或「下载全部」加入下载任务"
      action-text="去点播"
      @action="goVod"
    />

    <EmptyState
      v-else-if="filterEmpty"
      title="当前筛选下没有任务"
      desc="换「全部」或其它状态看看。"
      action-text="查看全部"
      @action="activeTab = 'all'"
    />

    <template v-else>
      <HintBar v-if="store.error" variant="error">刷新失败：{{ store.error }}</HintBar>

      <!-- 一部剧一张卡片：头部总进度 + 可折叠的每集进度 -->
      <div class="series-list" aria-label="下载剧集列表">
        <DownloadGroup
          v-for="group in groups"
          :key="group.key"
          :group="group"
          :expanded="expanded[group.key] === true"
          @toggle="onToggleGroup"
          @control="onControl"
          @remove="onRemove"
          @play="onPlay"
          @open-location="onOpenLocation"
          @bulk="onGroupBulk"
        />
      </div>
    </template>

    <!-- 删除已结束任务前的确认 -->
    <ConfirmDialog
      :open="confirmOpen"
      title="删除下载记录"
      :message="confirmMessage"
      confirm-text="删除"
      cancel-text="取消"
      danger
      checkbox-label="同时删除本地文件（不可恢复）"
      :checkbox="removeFile"
      @update:checkbox="removeFile = $event"
      @confirm="onConfirmRemove"
      @cancel="confirmOpen = false"
      @update:open="confirmOpen = $event"
    />
    </div>
  </PageScroll>
</template>

<style scoped>
.downloads-page .page-header {
  flex-wrap: wrap;
  gap: var(--space-4);
}

.toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2);
  min-width: 0;
}

.header-left {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-4);
  min-width: 0;
}

/* 左上角状态筛选 tab 样式在通用组件 StatusTabs 中 */

.stats {
  font-size: 12px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.stats-speed {
  color: var(--accent);
}

.dir {
  max-width: 360px;
  overflow: hidden;
  font-size: 12px;
  color: var(--text-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 剧集卡片列表：一部剧一张卡片，卡片之间留间距 */
.series-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}
</style>
