<!-- 设置页 · 历史记录分区：观看历史保留策略（时长 + 条数），修改后即时保存并触发后端裁剪 -->
<script setup lang="ts">
import { onMounted } from 'vue'
import { formatError } from '@/api'
import { useHistoryStore } from '@/store/history'
import { useUiStore } from '@/store/ui'

const history = useHistoryStore()
const ui = useUiStore()

/** 保存时长选项（值 = 天数，0 = 永久） */
const RETENTION_OPTIONS = [
  { label: '永久保存', value: 0 },
  { label: '最近 7 天', value: 7 },
  { label: '最近 30 天', value: 30 },
  { label: '最近 90 天', value: 90 },
  { label: '最近 180 天', value: 180 },
  { label: '最近 365 天', value: 365 },
]

/** 保存条数选项（值 = 条数，0 = 不限） */
const MAX_ITEMS_OPTIONS = [
  { label: '不限条数', value: 0 },
  { label: '最多 100 条', value: 100 },
  { label: '最多 200 条', value: 200 },
  { label: '最多 500 条', value: 500 },
  { label: '最多 1000 条', value: 1000 },
]

/** 保存策略：失败时用全局 toast 提示（后端保存后立即裁剪，成功后列表同步刷新） */
async function applySettings(patch: { retentionDays?: number; maxItems?: number }) {
  try {
    await history.updateSettings(patch)
    ui.notify('历史记录保留策略已更新', 'success')
  } catch (e) {
    ui.notify('更新保留策略失败：' + formatError(e), 'error')
  }
}

/** el-select 的 change 类型较宽，这里收窄到数字 */
function onRetentionChange(val: string | number | boolean | undefined) {
  if (typeof val === 'number') void applySettings({ retentionDays: val })
}

function onMaxItemsChange(val: string | number | boolean | undefined) {
  if (typeof val === 'number') void applySettings({ maxItems: val })
}

onMounted(() => {
  // 首次进入设置页：读取当前策略（历史页已加载过则直接复用）
  if (!history.settingsLoading) void history.loadSettings()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-history">
    <header class="section-head">
      <h2 id="settings-history" class="section-title">历史记录</h2>
      <p class="section-desc">观看历史保存在本机，可设置保存时长与保存条数。</p>
    </header>

    <div class="field-grid">
      <div class="field">
        <span class="field-label">保存时长</span>
        <span class="field-control">
          <el-select
            :model-value="history.settings.retentionDays"
            :disabled="history.settingsLoading"
            aria-label="历史记录保存时长"
            @change="onRetentionChange"
          >
            <el-option
              v-for="option in RETENTION_OPTIONS"
              :key="option.value"
              :label="option.label"
              :value="option.value"
            />
          </el-select>
        </span>
      </div>

      <div class="field">
        <span class="field-label">保存条数</span>
        <span class="field-control">
          <el-select
            :model-value="history.settings.maxItems"
            :disabled="history.settingsLoading"
            aria-label="历史记录保存条数"
            @change="onMaxItemsChange"
          >
            <el-option
              v-for="option in MAX_ITEMS_OPTIONS"
              :key="option.value"
              :label="option.label"
              :value="option.value"
            />
          </el-select>
        </span>
      </div>
    </div>

    <p class="section-note">修改后立即生效，超出保留范围的记录会自动清理。</p>
  </section>
</template>

<style scoped>
</style>
