<!-- 设置页 · 下载与缓存：下载目录（下载任务与播放缓存共用；Windows 默认「文档\VBox」） -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import * as api from '@/api'
import { formatBytes } from '@/utils/format'
import type { DownloadStorageStatus } from '@/types'
import { useDownloadsStore } from '@/store/downloads'
import { useUiStore } from '@/store/ui'

const store = useDownloadsStore()
const ui = useUiStore()

const changing = ref(false)
const opening = ref(false)
const restoring = ref(false)
const savingAutoSave = ref(false)
const savingNetwork = ref(false)
const savingAutoSavePolicy = ref(false)

/* ---------------- 空间管理 ---------------- */
const storage = ref<DownloadStorageStatus | null>(null)
const storageLoading = ref(false)
const cleaningParts = ref(false)
const cleaningCompleted = ref(false)
/** 待确认的清理动作：parts = .part 残留；completed = 已完成文件+记录 */
const cleanupAction = ref<'parts' | 'completed' | null>(null)

const diskPercent = computed(() => {
  const total = storage.value?.diskTotalBytes ?? 0
  const free = storage.value?.diskFreeBytes ?? 0
  if (total <= 0) return 0
  return Math.min(100, Math.max(0, Math.round(((total - free) / total) * 100)))
})

const cleanupTitle = computed(() =>
  cleanupAction.value === 'parts' ? '清理 .part 残留' : '清理已完成下载',
)
const cleanupMessage = computed(() => {
  if (cleanupAction.value === 'parts') {
    const count = storage.value?.partCount ?? 0
    return `将删除下载目录中的 ${count} 个未完成分片残留（*.part），不影响正常文件与播放缓存。`
  }
  const count = storage.value?.completedCount ?? 0
  return `将删除 ${count} 条已完成下载记录及其本地文件（不可恢复），请确认这些影片已不需要。`
})

/** 当前目录是否已是系统默认目录 */
const isDefault = computed(
  () => Boolean(store.settings.dir) && store.settings.dir === store.settings.defaultDir,
)

/** 写入新目录：后端返回生效后的设置 */
async function applyDir(dir: string) {
  const next = await api.downloadSetDir(dir)
  store.settings = next
  ui.notify(`下载目录已更新：${next.dir}`, 'success')
}

/** 选择目录：原生「选择文件夹」对话框，选中即保存 */
async function onChangeDir() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持选择本地目录', 'info')
    return
  }
  if (changing.value || opening.value || restoring.value) return
  changing.value = true
  try {
    const picked = await api.downloadPickDir()
    if (!picked) return // 用户取消
    if (picked === store.settings.dir) {
      ui.notify('目录未变化', 'info')
      return
    }
    await applyDir(picked)
  } catch (e) {
    ui.notify('更改目录失败：' + api.formatError(e), 'error')
  } finally {
    changing.value = false
  }
}

/** 恢复默认目录（文档\VBox / ~/Documents/VBox） */
async function onRestoreDefault() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持修改目录', 'info')
    return
  }
  const def = store.settings.defaultDir
  if (!def) {
    ui.notify('默认目录未知，请先刷新设置', 'warning')
    return
  }
  if (changing.value || opening.value || restoring.value) return
  restoring.value = true
  try {
    await applyDir(def)
  } catch (e) {
    ui.notify('恢复默认目录失败：' + api.formatError(e), 'error')
  } finally {
    restoring.value = false
  }
}

/** 打开当前下载目录（资源管理器 / Finder；demo 无本地目录） */
async function onOpenDir() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持打开本地目录', 'info')
    return
  }
  if (changing.value || opening.value || restoring.value) return
  opening.value = true
  try {
    await api.downloadOpenDir()
    ui.notify('已打开下载目录', 'info')
  } catch (e) {
    ui.notify('打开目录失败：' + api.formatError(e), 'error')
  } finally {
    opening.value = false
  }
}

/** 「播放时自动保存」开关：开启后播放会额外拉一份完整文件到下载目录 */
async function onAutoSaveChange(value: string | number | boolean) {
  if (typeof value !== 'boolean' || savingAutoSave.value) return
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持真实下载', 'info')
    return
  }
  savingAutoSave.value = true
  try {
    await store.updateAutoSave(value)
    ui.notify(
      value ? '已开启：播放时自动保存到下载目录' : '已关闭播放时自动保存',
      'success',
    )
  } catch (e) {
    ui.notify('设置失败：' + api.formatError(e), 'error')
  } finally {
    savingAutoSave.value = false
  }
}

/** 并发数 / 限速：保存到后端（对后续任务生效），成功后回填最新设置 */
async function onNetworkChange(patch: { concurrency?: number; speedLimitKbps?: number }) {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持真实下载', 'info')
    return
  }
  if (savingNetwork.value) return
  savingNetwork.value = true
  try {
    store.settings = await api.downloadSetNetwork(
      patch.concurrency ?? store.settings.concurrency,
      patch.speedLimitKbps ?? store.settings.speedLimitKbps,
    )
    ui.notify('下载设置已保存', 'success')
  } catch (e) {
    ui.notify('设置失败：' + api.formatError(e), 'error')
  } finally {
    savingNetwork.value = false
  }
}

function onConcurrencyChange(value: number | undefined) {
  void onNetworkChange({ concurrency: Math.min(8, Math.max(1, Math.trunc(Number(value) || 1))) })
}

function onSpeedLimitChange(value: number | undefined) {
  void onNetworkChange({
    speedLimitKbps: Math.max(0, Math.trunc(Number(value) || 0)),
  })
}

/** 自动保存策略：低优先级 / 单独限速（只影响非 HLS 的自动保存下载任务） */
async function saveAutoSavePolicy(lowPriority: boolean, autoSaveLimitKbps: number) {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持真实下载', 'info')
    return
  }
  if (savingAutoSavePolicy.value) return
  savingAutoSavePolicy.value = true
  try {
    store.settings = await api.downloadSetAutoSavePolicy(lowPriority, autoSaveLimitKbps)
    ui.notify('自动保存策略已保存', 'success')
  } catch (e) {
    ui.notify('设置失败：' + api.formatError(e), 'error')
  } finally {
    savingAutoSavePolicy.value = false
  }
}

function onLowPriorityChange(value: string | number | boolean) {
  void saveAutoSavePolicy(Boolean(value), store.settings.autoSaveLimitKbps)
}

function onAutoSaveLimitChange(value: number | undefined) {
  void saveAutoSavePolicy(
    store.settings.autoSaveLowPriority,
    Math.max(0, Math.trunc(Number(value) || 0)),
  )
}

/** 刷新空间总览（目录/缓存/.part/已完成 + 磁盘余量） */
async function loadStorage() {
  if (api.demoMode || storageLoading.value) return
  storageLoading.value = true
  try {
    storage.value = await api.downloadStorageStatus()
  } catch (e) {
    ui.notify('读取空间信息失败：' + api.formatError(e), 'error')
  } finally {
    storageLoading.value = false
  }
}

function openCleanup(action: 'parts' | 'completed') {
  cleanupAction.value = action
}

/** 执行清理：.part 残留 / 已完成下载（文件 + 记录） */
async function onConfirmCleanup() {
  const action = cleanupAction.value
  cleanupAction.value = null
  if (!action) return
  const cleaning = action === 'parts' ? cleaningParts : cleaningCompleted
  cleaning.value = true
  try {
    const result =
      action === 'parts'
        ? await api.downloadCleanupParts()
        : await api.downloadCleanupCompleted()
    ui.notify(
      `已清理 ${result.removedFiles} 个文件，释放 ${formatBytes(result.freedBytes)}`,
      'success',
    )
    if (action === 'completed') void store.refresh()
    await loadStorage()
  } catch (e) {
    ui.notify('清理失败：' + api.formatError(e), 'error')
  } finally {
    cleaning.value = false
  }
}

onMounted(() => {
  // 进入设置页时刷新一次（目录可能被下载页/其它入口修改过）
  void store.loadSettings()
  void loadStorage()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-download">
    <header class="section-head">
      <h2 id="settings-download" class="section-title">下载与缓存</h2>
      <p class="section-desc">
        下载任务与播放缓存共用该目录；修改后对新任务 / 新播放生效。
        下载文件按「影片名/集名」直接保存在该目录下（不再按站点分文件夹）。
        Windows 默认「文档\VBox」，macOS 默认「~/Documents/VBox」。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器预览模式：无法选择或打开本地目录，以下为演示占位。
    </HintBar>

    <div class="dir-field">
      <span class="field-label">下载目录</span>
        <span class="field-control dir-control">
          <el-input
            :model-value="store.settings.dir"
            class="dir-input"
            disabled
            placeholder="加载中…"
          />
          <el-button
            :loading="changing"
            :disabled="api.demoMode || opening || restoring"
            @click="onChangeDir"
          >
            <template #icon><SolarIcon name="folder-open" /></template>
            选择目录
          </el-button>
          <el-button
            :loading="opening"
            :disabled="api.demoMode || changing || restoring"
            @click="onOpenDir"
          >
            <template #icon><SolarIcon name="folder-open" /></template>
            打开目录
          </el-button>
          <el-button
            :loading="restoring"
            :disabled="api.demoMode || isDefault || changing || opening"
            @click="onRestoreDefault"
          >
            <template #icon><SolarIcon name="restart" /></template>
            恢复默认
          </el-button>
        </span>
        <p class="field-hint">
          当前生效：<code>{{ store.settings.dir || '加载中…' }}</code>
        </p>
    </div>

    <!-- 下载性能：并发数 / 全局限速 -->
    <div class="field net-field">
      <span class="field-label">下载性能</span>
      <div class="field-grid">
        <div class="field-control">
          <span class="switch-label">同时下载</span>
          <el-input-number
            :model-value="store.settings.concurrency"
            :min="1"
            :max="8"
            :step="1"
            :disabled="api.demoMode || savingNetwork"
            aria-label="同时下载任务数"
            @update:model-value="onConcurrencyChange"
          />
          <span class="switch-label">路</span>
        </div>
        <div class="field-control">
          <span class="switch-label">限速</span>
          <el-input-number
            :model-value="store.settings.speedLimitKbps"
            :min="0"
            :max="1048576"
            :step="128"
            :disabled="api.demoMode || savingNetwork"
            aria-label="全局下载限速 KB/s"
            @update:model-value="onSpeedLimitChange"
          />
          <span class="switch-label">KB/s（0 = 不限速）</span>
        </div>
      </div>
      <p class="field-hint">
        并发数越大越吃带宽；限速对全部下载任务生效，修改后对后续任务立即生效。
      </p>
    </div>

    <!-- 自动保存策略：低优先级让路 + 单独限速 -->
    <div class="field auto-save-policy">
      <span class="field-label">自动保存策略</span>
      <div class="field-grid">
        <div class="field-control">
          <el-switch
            :model-value="store.settings.autoSaveLowPriority"
            :loading="savingAutoSavePolicy"
            :disabled="api.demoMode"
            aria-label="自动保存低优先级"
            @change="onLowPriorityChange"
          />
          <span class="switch-label">低优先级（手动下载进行时自动让路）</span>
        </div>
        <div class="field-control">
          <span class="switch-label">限速</span>
          <el-input-number
            :model-value="store.settings.autoSaveLimitKbps"
            :min="0"
            :max="1048576"
            :step="128"
            :disabled="api.demoMode || savingAutoSavePolicy"
            aria-label="自动保存限速 KB/s"
            @update:model-value="onAutoSaveLimitChange"
          />
          <span class="switch-label">KB/s（0 = 不额外限制）</span>
        </div>
      </div>
      <p class="field-hint">
        仅对非 HLS 的自动保存下载任务生效；HLS 边看边存复用播放缓存，本身只有一份流量。
      </p>
    </div>

    <!-- 空间管理：占用总览 + .part/已完成清理 -->
    <div class="field storage-field">
      <span class="field-label">空间管理</span>
      <div class="storage-grid">
        <div class="storage-item">
          <span class="storage-key">下载目录占用</span>
          <strong class="storage-value">{{ formatBytes(storage?.usedBytes) }}</strong>
        </div>
        <div class="storage-item">
          <span class="storage-key">播放缓存</span>
          <strong class="storage-value">{{ formatBytes(storage?.cacheBytes) }}</strong>
        </div>
        <div class="storage-item">
          <span class="storage-key">.part 残留</span>
          <strong class="storage-value">
            {{ formatBytes(storage?.partBytes) }}
            <span class="storage-sub">（{{ storage?.partCount ?? 0 }} 个）</span>
          </strong>
        </div>
        <div class="storage-item">
          <span class="storage-key">已完成文件</span>
          <strong class="storage-value">
            {{ formatBytes(storage?.completedBytes) }}
            <span class="storage-sub">（{{ storage?.completedCount ?? 0 }} 个）</span>
          </strong>
        </div>
        <div class="storage-item">
          <span class="storage-key">磁盘剩余</span>
          <strong class="storage-value">
            {{ formatBytes(storage?.diskFreeBytes) }}
            <span class="storage-sub">
              / {{ storage && storage.diskTotalBytes > 0 ? formatBytes(storage.diskTotalBytes) : '未知' }}
            </span>
          </strong>
        </div>
      </div>
      <el-progress
        v-if="storage && storage.diskTotalBytes > 0"
        class="storage-progress"
        :percentage="diskPercent"
        :stroke-width="6"
        :show-text="false"
      />
      <div class="storage-actions">
        <el-button :loading="storageLoading" :disabled="api.demoMode" @click="loadStorage">
          <template #icon><SolarIcon name="refresh" /></template>
          刷新
        </el-button>
        <el-button
          :loading="cleaningParts"
          :disabled="api.demoMode || !storage || storage.partCount === 0"
          @click="openCleanup('parts')"
        >
          <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
          清理 .part 残留
        </el-button>
        <el-button
          type="danger"
          :loading="cleaningCompleted"
          :disabled="api.demoMode || !storage || storage.completedCount === 0"
          @click="openCleanup('completed')"
        >
          <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
          清理已完成下载
        </el-button>
      </div>
      <p class="field-hint">
        清理已完成下载会同时删除记录与本地文件；播放缓存的清理在「播放缓存」分区单独管理。
      </p>
    </div>

    <!-- 播放时自动保存：边播边存（额外一份带宽，换取完整可播放文件） -->
    <div class="switch-row">
      <el-switch
        :model-value="store.settings.autoSave"
        :loading="savingAutoSave"
        :disabled="api.demoMode"
        aria-label="播放时自动保存"
        @change="onAutoSaveChange"
      />
      <div class="switch-text">
        <span class="field-label">播放时自动保存</span>
        <p class="switch-desc">
          播放时把当前集另存到下载目录。这是全局默认值；播放器控制条上的「边下边播」可对单个影片单独开关。
        </p>
      </div>
    </div>

    <p class="section-note">
      默认目录：<span class="default-dir">{{ store.settings.defaultDir || '（未获取）' }}</span>。修改目录不影响已开始的任务。
    </p>

    <!-- 清理前确认 -->
    <ConfirmDialog
      :open="cleanupAction !== null"
      :title="cleanupTitle"
      :message="cleanupMessage"
      :confirm-text="cleanupAction === 'parts' ? '清理' : '删除'"
      cancel-text="取消"
      :danger="cleanupAction === 'completed'"
      @confirm="onConfirmCleanup"
      @cancel="cleanupAction = null"
      @update:open="cleanupAction = $event ? cleanupAction : null"
    />
  </section>
</template>

<style scoped>
/* 下载目录：单行（输入框 + 按钮），不再使用两列 field-grid */
.dir-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
}

/* 目录行：输入框 flex:1 + 三个按钮，单行显示 */
.dir-control {
  flex-wrap: nowrap;
}

.dir-input {
  flex: 1;
  min-width: 0;
}

/* 下载性能 / 自动保存策略的数字输入框统一宽度（Element Plus 默认 150px 会显得更宽） */
.net-field :deep(.el-input-number),
.auto-save-policy :deep(.el-input-number) {
  width: 132px;
}

.storage-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
  gap: var(--space-2);
  margin-top: var(--space-2);
}

.storage-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--space-2) var(--space-3);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.storage-key {
  font-size: 12px;
  color: var(--text-muted);
}

.storage-value {
  font-size: 13px;
  color: var(--text);
}

.storage-sub {
  font-size: 11.5px;
  font-weight: 400;
  color: var(--text-faint);
}

.storage-progress {
  margin-top: var(--space-2);
}

.storage-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: var(--space-2);
}

.field-hint code {
  padding: 1px var(--space-1);
  border-radius: var(--radius-sm);
  background: var(--surface-2);
  font-size: 11px;
}

/* 播放自动保存开关：左侧开关 + 右侧说明 */
.switch-row {
  display: flex;
  align-items: flex-start;
  gap: var(--space-3);
  margin-top: var(--space-4);
  padding: var(--space-3);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.switch-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.switch-desc {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--text-muted);
}

.default-dir {
  font-family: var(--font-mono);
  color: var(--text-muted);
  user-select: text;
}
</style>
