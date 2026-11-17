<!-- 设置页 · 播放缓存：保留策略（大小上限 / 保留天数，均可无上限）+ 当前占用 + 清除缓存。
     缓存位于 <下载目录>/cache/hls/，跨重启保留；仅播放边下边播使用，不影响已下载文件。 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { PlaybackCachePrefs, PlaybackCacheStatus } from '@/types'
import { formatBytes } from '@/utils/format'

const GB = 1024 * 1024 * 1024
/** 打开上限但还没填值时使用的默认值 */
const DEFAULT_MAX_GB = 2
const DEFAULT_DAYS = 7

const ui = useUiStore()

const loading = ref(false)
const saving = ref(false)
const clearing = ref(false)
const refreshing = ref(false)

/** 已保存的策略（字节 / 天数，0 = 不限制） */
const prefs = ref<PlaybackCachePrefs>({ maxBytes: 0, retentionDays: 0 })
/** 当前占用 */
const status = ref<PlaybackCacheStatus | null>(null)

/** 控件状态：无上限开关 + 输入框数值 */
const maxUnlimited = ref(true)
const maxGb = ref(DEFAULT_MAX_GB)
const daysUnlimited = ref(true)
const days = ref(DEFAULT_DAYS)

const sizeText = computed(() => formatBytes(status.value?.bytes ?? 0))
const filesText = computed(() => `${status.value?.files ?? 0} 个文件`)
const dirText = computed(() => status.value?.dir || '加载中…')
const policyText = computed(() => {
  const size = maxUnlimited.value ? '大小不限制' : `大小上限 ${maxGb.value.toFixed(1)} GB`
  const age = daysUnlimited.value ? '时间不限制' : `保留 ${days.value} 天`
  return `${size} · ${age}`
})

/** 用后端返回的策略同步控件状态 */
function syncFromPrefs(next: PlaybackCachePrefs) {
  prefs.value = next
  maxUnlimited.value = next.maxBytes === 0
  maxGb.value =
    next.maxBytes > 0 ? Math.round((next.maxBytes / GB) * 10) / 10 : DEFAULT_MAX_GB
  daysUnlimited.value = next.retentionDays === 0
  days.value = next.retentionDays > 0 ? next.retentionDays : DEFAULT_DAYS
}

async function load() {
  if (loading.value) return
  loading.value = true
  try {
    syncFromPrefs(await api.playbackCachePrefs())
    status.value = await api.playbackCacheStatus()
  } catch (e) {
    ui.notify('读取播放缓存设置失败：' + api.formatError(e), 'error')
  } finally {
    loading.value = false
  }
}

async function refreshStatus() {
  if (refreshing.value) return
  refreshing.value = true
  try {
    status.value = await api.playbackCacheStatus()
  } catch (e) {
    ui.notify('读取缓存占用失败：' + api.formatError(e), 'error')
  } finally {
    refreshing.value = false
  }
}

/** 保存策略；后端保存后会立即按新策略清理一次 */
async function save(patch: Partial<PlaybackCachePrefs>) {
  if (saving.value) return
  saving.value = true
  try {
    syncFromPrefs(await api.playbackCacheSetPrefs({ ...prefs.value, ...patch }))
    status.value = await api.playbackCacheStatus()
    ui.notify('播放缓存设置已保存（已按新策略清理）', 'success')
  } catch (e) {
    ui.notify('保存播放缓存设置失败：' + api.formatError(e), 'error')
    await load()
  } finally {
    saving.value = false
  }
}

/** 大小上限：无上限开关 */
function onToggleMaxUnlimited(value: string | number | boolean) {
  if (saving.value) return
  const unlimited = Boolean(value)
  maxUnlimited.value = unlimited
  const gb = maxGb.value > 0 ? maxGb.value : DEFAULT_MAX_GB
  void save({ maxBytes: unlimited ? 0 : Math.round(gb * GB) })
}

/** 大小上限：GB 输入 */
function onMaxGbChange(value: number | undefined) {
  if (saving.value || typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return
  maxGb.value = value
  void save({ maxBytes: Math.round(value * GB) })
}

/** 保留天数：无上限开关 */
function onToggleDaysUnlimited(value: string | number | boolean) {
  if (saving.value) return
  const unlimited = Boolean(value)
  daysUnlimited.value = unlimited
  const day = days.value > 0 ? days.value : DEFAULT_DAYS
  void save({ retentionDays: unlimited ? 0 : day })
}

/** 保留天数输入 */
function onDaysChange(value: number | undefined) {
  if (
    saving.value ||
    typeof value !== 'number' ||
    !Number.isFinite(value) ||
    value <= 0
  ) {
    return
  }
  days.value = Math.round(value)
  void save({ retentionDays: Math.round(value) })
}

/** 清空播放缓存（不影响已下载文件） */
async function onClear() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式没有本地播放缓存', 'info')
    return
  }
  if (clearing.value) return
  clearing.value = true
  const before = status.value?.bytes ?? 0
  try {
    status.value = await api.playbackCacheClear()
    ui.notify(`已清除播放缓存，释放 ${formatBytes(before)}`, 'success')
  } catch (e) {
    ui.notify('清除缓存失败：' + api.formatError(e), 'error')
  } finally {
    clearing.value = false
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-playback-cache">
    <header class="section-head">
      <h2 id="settings-playback-cache" class="section-title">播放缓存</h2>
      <p class="section-desc">
        播放 HLS 时边下边播的分片缓存会保留在下载目录的 <code>cache</code> 子目录下，重新打开同一集可直接复用。
        这里控制它保留多久、最多占多少空间；两项都可设为「无上限」（此时不会自动清理）。
        只影响播放缓存，不影响已下载的影片文件。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器预览模式：没有真实播放缓存，以下为演示占位。
    </HintBar>

    <!-- 当前占用 -->
    <div class="cache-usage">
      <div class="usage-main">
        <span class="usage-value">{{ sizeText }}</span>
        <span class="usage-files">{{ filesText }}</span>
      </div>
      <el-button :loading="refreshing" :disabled="loading" @click="refreshStatus">
        <template #icon><SolarIcon name="restart" /></template>
        刷新
      </el-button>
    </div>
    <p class="field-hint">缓存目录：<code>{{ dirText }}</code></p>

    <!-- 策略 -->
    <div class="field-grid cache-fields">
      <div class="field">
        <span class="field-label">大小上限</span>
        <div class="field-control cache-control">
          <el-switch
            :model-value="maxUnlimited"
            :disabled="loading || saving"
            aria-label="播放缓存大小无上限"
            @change="onToggleMaxUnlimited"
          />
          <span class="switch-label">{{ maxUnlimited ? '无上限' : '限制为' }}</span>
          <el-input-number
            v-if="!maxUnlimited"
            :model-value="maxGb"
            :min="0.1"
            :max="100"
            :step="0.5"
            :precision="1"
            :disabled="loading || saving"
            controls-position="right"
            aria-label="播放缓存大小上限（GB）"
            @change="onMaxGbChange"
          />
          <span v-if="!maxUnlimited" class="unit">GB</span>
        </div>
      </div>

      <div class="field">
        <span class="field-label">保留天数</span>
        <div class="field-control cache-control">
          <el-switch
            :model-value="daysUnlimited"
            :disabled="loading || saving"
            aria-label="播放缓存时间无上限"
            @change="onToggleDaysUnlimited"
          />
          <span class="switch-label">{{ daysUnlimited ? '无上限' : '保留' }}</span>
          <el-input-number
            v-if="!daysUnlimited"
            :model-value="days"
            :min="1"
            :max="3650"
            :step="1"
            :precision="0"
            :disabled="loading || saving"
            controls-position="right"
            aria-label="播放缓存保留天数"
            @change="onDaysChange"
          />
          <span v-if="!daysUnlimited" class="unit">天</span>
        </div>
      </div>
    </div>

    <div class="cache-actions">
      <el-popconfirm
        title="确定清除全部播放缓存？不影响已下载的影片文件。"
        confirm-button-text="清除"
        cancel-button-text="取消"
        width="260"
        @confirm="onClear"
      >
        <template #reference>
          <el-button type="danger" plain :loading="clearing" :disabled="api.demoMode || loading">
            <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
            清除播放缓存
          </el-button>
        </template>
      </el-popconfirm>
      <span class="policy-text">当前策略：{{ policyText }}</span>
    </div>

    <p class="section-note">
      设为「无上限」时不会自动清理，建议按磁盘空间给一个上限。
    </p>
  </section>
</template>

<style scoped>
.section-desc code,
.field-hint code {
  padding: 1px var(--space-1);
  border-radius: var(--radius-sm);
  background: var(--surface-2);
  font-size: 11px;
}

.cache-usage {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  margin-top: var(--space-3);
  padding: var(--space-3);
  border: 1px solid var(--border-lighter);
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.usage-main {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: 0;
}

.usage-value {
  font-size: 20px;
  font-weight: 600;
  color: var(--text);
  font-variant-numeric: tabular-nums;
}

.usage-files {
  font-size: 12px;
  color: var(--text-muted);
}

.cache-fields {
  margin-top: var(--space-4);
}

.cache-control {
  flex-wrap: wrap;
}

.cache-control :deep(.el-input-number) {
  width: 120px;
}

.unit {
  font-size: 12px;
  color: var(--text-muted);
}

.cache-actions {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-top: var(--space-4);
  flex-wrap: wrap;
}

.policy-text {
  font-size: 12px;
  color: var(--text-muted);
}

</style>
