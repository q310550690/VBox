<!-- 独立运行日志窗口（Tauri 单独 WebviewWindow：index.html#/log-window）
     用于调试：自动刷新、级别过滤、关键字过滤、单行复制、清空、导出 -->
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
const autoRefresh = ref(true)
const keyword = ref('')
const level = ref<'all' | 'info' | 'warn' | 'error'>('all')
const error = ref('')
const copiedId = ref<string | null>(null)
let timer: number | null = null
let copiedTimer: number | null = null

const filtered = computed(() => {
  const text = keyword.value.trim().toLowerCase()
  return logs.value.filter((entry) => {
    if (level.value !== 'all' && entry.level !== level.value) return false
    if (!text) return true
    return entry.text.toLowerCase().includes(text)
  })
})

function pad(n: number) {
  return String(n).padStart(2, '0')
}

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

async function copyLine(entry: LogEntry) {
  const key = `${entry.at}-${entry.text}`
  try {
    await navigator.clipboard.writeText(`${formatTime(entry.at)} [${entry.level}] ${entry.text}`)
    copiedId.value = key
    if (copiedTimer !== null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copiedId.value = null
      copiedTimer = null
    }, 1200)
  } catch {
    // 剪贴板不可用时忽略
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

function syncTimer() {
  if (timer !== null) {
    window.clearInterval(timer)
    timer = null
  }
  if (autoRefresh.value && !api.demoMode) {
    timer = window.setInterval(() => void load(), 1000)
  }
}

onMounted(() => {
  void load()
  syncTimer()
})

onBeforeUnmount(() => {
  if (timer !== null) window.clearInterval(timer)
  if (copiedTimer !== null) window.clearTimeout(copiedTimer)
  timer = null
  copiedTimer = null
})
</script>

<template>
  <div class="log-window">
    <header class="log-header">
      <div class="title-row">
        <h1 class="log-title">VBox 运行日志</h1>
        <span class="log-count">共 {{ logs.length }} 条（显示 {{ filtered.length }} 条）</span>
      </div>
      <div class="actions">
        <el-switch v-model="autoRefresh" active-text="自动刷新" @change="syncTimer" />
        <el-select v-model="level" class="level-select" aria-label="日志级别过滤">
          <el-option label="全部级别" value="all" />
          <el-option label="info" value="info" />
          <el-option label="warn" value="warn" />
          <el-option label="error" value="error" />
        </el-select>
        <el-input
          v-model="keyword"
          class="keyword"
          clearable
          placeholder="按关键字过滤…"
        />
        <el-button :loading="loading" @click="load">
          <template #icon><SolarIcon name="refresh" :size="14" /></template>
          刷新
        </el-button>
        <el-button :loading="exporting" @click="onExport">
          <template #icon><SolarIcon name="download" :size="14" /></template>
          导出
        </el-button>
        <el-button :loading="clearing" :disabled="!logs.length" @click="onClear">
          <template #icon><SolarIcon name="trash-bin-minimalistic" :size="14" /></template>
          清空
        </el-button>
      </div>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器预览模式：日志为空；桌面端才会记录运行日志。
    </HintBar>
    <HintBar v-if="error" variant="error">读取日志失败：{{ error }}</HintBar>

    <p v-else-if="!filtered.length" class="empty">暂无匹配的日志。</p>

    <div v-else class="log-list" role="log">
      <div v-for="(entry, index) in filtered" :key="`${entry.at}-${index}`" class="log-row" :class="`is-${entry.level}`">
        <span class="log-time">{{ formatTime(entry.at) }}</span>
        <span class="log-level">{{ entry.level }}</span>
        <span class="log-text">{{ entry.text }}</span>
        <button
          type="button"
          class="log-copy"
          :title="copiedId === `${entry.at}-${entry.text}` ? '已复制' : '复制这一行'"
          @click="copyLine(entry)"
        >
          <SolarIcon :name="copiedId === `${entry.at}-${entry.text}` ? 'check-circle' : 'copy'" :size="12" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.log-window {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  box-sizing: border-box;
  height: 100vh;
  padding: var(--space-3);
  background: var(--content-bg);
  color: var(--text);
}

.log-header {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  flex-shrink: 0;
}

.title-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
}

.log-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.log-count {
  font-size: 12px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}

.level-select {
  width: 130px;
}

.keyword {
  flex: 1 1 220px;
  min-width: 0;
}

.log-list {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  overflow: auto;
  padding: var(--space-2);
  border: 1px solid var(--border-lighter);
  border-radius: var(--radius-md);
  background: var(--surface-2);
  font-family: var(--font-mono);
}

.log-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: max-content;
  font-size: 11.5px;
  line-height: 1.55;
}

.log-row:hover {
  background: var(--surface-3);
}

.log-time {
  flex-shrink: 0;
  font-size: 10.5px;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.log-level {
  flex-shrink: 0;
  min-width: 38px;
  color: var(--text-muted);
  font-size: 10px;
  text-transform: uppercase;
}

.log-row.is-warn .log-level {
  color: var(--warning);
}

.log-row.is-error .log-level {
  color: var(--danger);
}

.log-text {
  white-space: pre;
  color: var(--text-regular);
}

.log-copy {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
  opacity: 0;
}

.log-row:hover .log-copy {
  opacity: 1;
}

.log-copy:hover {
  background: var(--surface-4);
  color: var(--text);
}

.empty {
  margin: 0;
  padding: var(--space-3);
  color: var(--text-muted);
  font-size: 12px;
}
</style>
