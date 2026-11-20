<!-- 设置页 · 运行日志：后端内存环形日志（播放/保存/代理/下载等关键事件）+ 导出 -->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { LogEntry } from '@/types'

const ui = useUiStore()

const logs = ref<LogEntry[]>([])
const loading = ref(false)
const clearing = ref(false)
const exporting = ref(false)
const error = ref('')
let timer: number | null = null

/** 仅展示最近 300 条，避免 DOM 过多 */
const visibleLogs = computed(() => logs.value.slice(-300))

function pad(n: number) {
  return String(n).padStart(2, '0')
}

/** Unix 毫秒 → HH:mm:ss.SSS */
function formatTime(ms: number): string {
  const date = new Date(ms)
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${String(
    date.getMilliseconds(),
  ).padStart(3, '0')}`
}

async function load() {
  if (api.demoMode || loading.value) return
  loading.value = true
  error.value = ''
  try {
    logs.value = await api.logsRecent(500)
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

async function onOpenWindow() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持独立日志窗口', 'info')
    return
  }
  try {
    await api.openLogWindow()
  } catch (e) {
    ui.notify('打开日志窗口失败：' + api.formatError(e), 'error')
  }
}

async function onExport() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持导出日志', 'info')
    return
  }
  if (exporting.value) return
  exporting.value = true
  try {
    const path = await api.logsExport()
    if (path) ui.notify(`日志已导出：${path}`, 'success')
  } catch (e) {
    ui.notify('导出失败：' + api.formatError(e), 'error')
  } finally {
    exporting.value = false
  }
}

async function onClear() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持清空日志', 'info')
    return
  }
  if (clearing.value) return
  clearing.value = true
  try {
    await api.logsClear()
    logs.value = []
    ui.notify('已清空运行日志', 'success')
  } catch (e) {
    ui.notify('清空失败：' + api.formatError(e), 'error')
  } finally {
    clearing.value = false
  }
}

onMounted(() => {
  void load()
  if (!api.demoMode) timer = window.setInterval(() => void load(), 3000)
})

onBeforeUnmount(() => {
  if (timer !== null) window.clearInterval(timer)
  timer = null
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-logs">
    <header class="section-head">
      <h2 id="settings-logs" class="section-title">运行日志</h2>
      <p class="section-desc">
        VBox 关键事件（播放状态 / HLS 会话 / 保存 / 下载 / 代理等）的内存日志，最多保留 500 条，重启清空。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器预览模式：运行日志为空；桌面端（Tauri）才会记录并允许导出。
    </HintBar>

    <div class="log-toolbar">
      <el-button :loading="loading" @click="load">
        <template #icon><SolarIcon name="refresh" :size="14" /></template>
        刷新
      </el-button>
      <el-button :disabled="api.demoMode" @click="onOpenWindow">
        <template #icon><SolarIcon name="maximize-square" :size="14" /></template>
        独立窗口
      </el-button>
      <el-button :loading="exporting" @click="onExport">
        <template #icon><SolarIcon name="download" :size="14" /></template>
        导出日志
      </el-button>
      <el-button :loading="clearing" :disabled="!logs.length" @click="onClear">
        <template #icon><SolarIcon name="trash-bin-minimalistic" :size="14" /></template>
        清空
      </el-button>
      <span class="log-count muted">共 {{ logs.length }} 条日志</span>
    </div>

    <HintBar v-if="error" variant="error">读取运行日志失败：{{ error }}</HintBar>

    <!-- 空态判断不要掺 loading：否则每次刷新（含 3s 定时）空态/日志框来回切换，页面会闪 -->
    <p v-else-if="!visibleLogs.length" class="log-empty">
      暂无运行日志。开始播放 / 保存 / 下载后，这里会出现真实运行记录。
    </p>

    <el-scrollbar v-else max-height="360px" class="log-scroll">
      <div class="log-list" role="log">
        <div v-for="(entry, index) in visibleLogs" :key="index" class="log-row" :class="`is-${entry.level}`">
          <span class="log-time">{{ formatTime(entry.at) }}</span>
          <span class="log-level">{{ entry.level }}</span>
          <span class="log-text">{{ entry.text }}</span>
        </div>
      </div>
    </el-scrollbar>
  </section>
</template>

<style scoped>
.log-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}

.log-count {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.log-empty {
  margin: 0;
  padding: var(--space-2);
  color: var(--text-muted);
  font-size: 12px;
}

.log-scroll {
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
  margin: 0;
  padding: 0;
}

.log-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--space-2);
}

.log-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: max-content;
  font-size: 11.5px;
  line-height: 1.5;
}

.log-time {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 10.5px;
  font-variant-numeric: tabular-nums;
  color: var(--text-faint);
}

.log-level {
  flex-shrink: 0;
  min-width: 34px;
  padding: 0 var(--space-1);
  border-radius: var(--radius-sm);
  background: var(--surface-3);
  color: var(--text-muted);
  font-size: 10px;
  text-align: center;
  text-transform: uppercase;
}

.log-row.is-warn .log-level {
  color: var(--warning);
}

.log-row.is-error .log-level {
  color: var(--danger);
}

.log-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--text-regular);
}
</style>
