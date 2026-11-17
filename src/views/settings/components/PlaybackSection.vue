<!-- 设置页 · 播放分区：硬解 / 默认倍速（持久化到 <app_data_dir>/player.json）
     音量不再是设置项：由播放器在关闭播放 / 退出应用时自动记住上次调节值，初始默认 100%。 -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { PlaybackPrefs } from '@/types'

const ui = useUiStore()

/** 当前设置（后端归一化后回填）；volume 仅随设置回传，页面不再展示/编辑 */
const prefs = ref<PlaybackPrefs>({ hwdec: 'auto-safe', volume: 100, rate: 1 })
const loading = ref(false)
const saving = ref(false)

/** 硬解选项：与后端白名单一致（非法值后端会回退 auto-safe） */
const HWDEC_OPTIONS = [
  { label: '自动（推荐）', value: 'auto-safe' },
  { label: 'D3D11VA 硬解', value: 'd3d11va' },
  { label: '自动（兼容旧设备）', value: 'auto' },
  { label: '关闭硬解（软解）', value: 'no' },
] as const

/** 默认倍速选项 */
const RATE_OPTIONS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const

async function load() {
  if (loading.value) return
  loading.value = true
  try {
    prefs.value = await api.playerPrefs()
  } catch (e) {
    ui.notify('读取播放设置失败：' + api.formatError(e), 'error')
  } finally {
    loading.value = false
  }
}

/** 保存进行中又被拖动时暂存补丁，等当前保存结束再落盘一次（不丢用户的最后一次调整） */
let queuedPatch: Partial<PlaybackPrefs> | null = null

/** 保存整份设置（失败时回滚到后端当前值） */
async function save(patch: Partial<PlaybackPrefs>) {
  if (saving.value) {
    queuedPatch = { ...(queuedPatch ?? {}), ...patch }
    return
  }
  saving.value = true
  try {
    prefs.value = await api.playerSetPrefs({ ...prefs.value, ...patch })
    ui.notify('播放设置已保存（下次打开播放器生效）', 'success')
  } catch (e) {
    ui.notify('保存播放设置失败：' + api.formatError(e), 'error')
    await load()
  } finally {
    saving.value = false
    const pending = queuedPatch
    queuedPatch = null
    if (pending) void save(pending)
  }
}

function onHwdecChange(value: string | number | boolean | undefined) {
  if (typeof value === 'string') void save({ hwdec: value })
}

function onRateChange(value: string | number | boolean | undefined) {
  const rate = Number(value)
  if (Number.isFinite(rate) && rate > 0) void save({ rate })
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-playback">
    <header class="section-head">
      <h2 id="settings-playback" class="section-title">播放</h2>
      <p class="section-desc">
        播放器（libmpv）默认参数；保存后对下次打开播放器生效，当前播放可在控制条上临时切换倍速。
        音量不需要设置：会自动记住你上次关闭播放器 / 退出应用时的音量，初始为 100%。
      </p>
    </header>

    <div class="field-grid">
      <div class="field">
        <span class="field-label">硬解 / 软解</span>
        <span class="field-control">
          <el-select
            :model-value="prefs.hwdec"
            :disabled="loading || saving"
            aria-label="硬解或软解"
            @change="onHwdecChange"
          >
            <el-option
              v-for="option in HWDEC_OPTIONS"
              :key="option.value"
              :label="option.label"
              :value="option.value"
            />
          </el-select>
        </span>
      </div>

      <div class="field">
        <span class="field-label">默认倍速</span>
        <span class="field-control">
          <el-select
            :model-value="prefs.rate"
            :disabled="loading || saving"
            aria-label="默认倍速"
            @change="onRateChange"
          >
            <el-option
              v-for="rate in RATE_OPTIONS"
              :key="rate"
              :label="`${rate}x`"
              :value="rate"
            />
          </el-select>
        </span>
      </div>
    </div>

    <p class="section-note">
      硬解失败会自动回退软解；字幕/音轨在播放控制条切换。
    </p>
  </section>
</template>

<style scoped>
</style>
