<!-- 设置页 · 网络与抓包（最小可用）：VBox 自身请求日志 + WebView 轻量抓包测试
     说明：不是系统级全量抓包，也不做 HTTPS MITM；日志来自 http 模块，嗅探来自隐藏 WebView 的
     fetch/XHR/performance 钩子，本地文件/代理由 127.0.0.1 的 /file 与 /proxy 提供。 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { NetworkEvent, PlayResult } from '@/types'

const ui = useUiStore()

/* ---------------- 请求日志 ---------------- */

const events = ref<NetworkEvent[]>([])
const logLoading = ref(false)
const logClearing = ref(false)
const logError = ref('')

async function loadLog() {
  if (logLoading.value) return
  logLoading.value = true
  logError.value = ''
  try {
    events.value = await api.networkLog(100)
  } catch (e) {
    logError.value = api.formatError(e)
  } finally {
    logLoading.value = false
  }
}

async function clearLog() {
  if (logClearing.value || !events.value.length) return
  logClearing.value = true
  try {
    await api.networkClear()
    events.value = []
    ui.notify('已清空请求日志', 'success')
  } catch (e) {
    ui.notify('清空失败：' + api.formatError(e), 'error')
  } finally {
    logClearing.value = false
  }
}

function formatDuration(ms?: number | null): string {
  if (typeof ms !== 'number' || !Number.isFinite(ms)) return '—'
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${ms}ms`
}

function formatSize(size?: number | null): string {
  if (typeof size !== 'number' || !Number.isFinite(size)) return '—'
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)}MB`
  if (size >= 1024) return `${(size / 1024).toFixed(1)}KB`
  return `${size}B`
}

/* ---------------- 抓包测试 ---------------- */

const sniffUrl = ref('')
const sniffing = ref(false)
const sniffResult = ref<PlayResult | null>(null)
const sniffError = ref('')

const sniffedHeaders = computed<string[]>(() =>
  Object.entries(sniffResult.value?.headers ?? {}).map(([key, value]) => `${key}: ${value}`),
)
const sniffTrace = computed<string[]>(() => sniffResult.value?.trace ?? [])

async function runSniff() {
  const url = sniffUrl.value.trim()
  if (!url) {
    ui.notify('请先填写播放页 URL', 'warning')
    return
  }
  if (!/^https?:\/\//i.test(url)) {
    ui.notify('抓包仅支持 http(s) 播放页地址', 'warning')
    return
  }
  if (sniffing.value) return
  sniffing.value = true
  sniffResult.value = null
  sniffError.value = ''
  try {
    const result = await api.sniff({ url })
    sniffResult.value = result
    if (!result.url) sniffError.value = '抓包完成但没有发现媒体地址（页面可能没有加载播放器）'
  } catch (e) {
    sniffError.value = api.formatError(e)
  } finally {
    sniffing.value = false
  }
}

onMounted(() => {
  void loadLog()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-network">
    <header class="section-head">
      <h2 id="settings-network" class="section-title">网络与抓包</h2>
      <p class="section-desc">
        查看 VBox 自身发出的请求，并用隐藏 WebView 嗅探播放页里的真实媒体地址（轻量抓包，非系统级全量抓包）。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：请求日志为空，抓包测试不可用；桌面端（Tauri）才会真正调用后端命令。
    </HintBar>

    <!-- 请求日志 -->
    <div class="net-toolbar">
      <span class="net-title">最近请求（最多 100 条）</span>
      <span class="net-spacer" />
      <el-button :loading="logLoading" @click="loadLog">
        <template #icon><SolarIcon name="refresh" /></template>
        刷新
      </el-button>
      <el-button
        :disabled="!events.length"
        :loading="logClearing"
        @click="clearLog"
      >
        <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
        清空
      </el-button>
    </div>

    <HintBar v-if="logError" variant="error">读取请求日志失败：{{ logError }}</HintBar>
    <p v-else-if="!events.length && !logLoading" class="net-empty">
      暂无请求日志。加载订阅、列表、详情或解析后，这里会记录 VBox 自身发出的 HTTP 请求。
    </p>
    <el-table v-else class="net-table" :data="events" size="small" max-height="280" border>
      <el-table-column prop="kind" label="类型" width="82" />
      <el-table-column prop="method" label="方法" width="72" />
      <el-table-column label="URL" min-width="260">
        <template #default="{ row }">
          <span class="net-url" :title="row.url">{{ row.url }}</span>
        </template>
      </el-table-column>
      <el-table-column label="状态" width="70">
        <template #default="{ row }">{{ row.status ?? '—' }}</template>
      </el-table-column>
      <el-table-column label="耗时" width="82">
        <template #default="{ row }">{{ formatDuration(row.durationMs) }}</template>
      </el-table-column>
      <el-table-column label="大小" width="82">
        <template #default="{ row }">{{ formatSize(row.size) }}</template>
      </el-table-column>
      <el-table-column label="错误" min-width="120">
        <template #default="{ row }">
          <span class="net-error" :title="row.error || ''">{{ row.error || '—' }}</span>
        </template>
      </el-table-column>
    </el-table>

    <!-- 抓包测试 -->
    <div class="sniff-form">
      <span class="net-title">抓包测试</span>
      <el-input
        v-model="sniffUrl"
        class="sniff-input"
        placeholder="播放页 URL（http/https，例如完整播放页/解析页）"
        clearable
        @keyup.enter="runSniff"
      />
      <el-button
        type="primary"
        :loading="sniffing"
        :disabled="api.demoMode"
        @click="runSniff"
      >
        <template #icon><SolarIcon name="videocamera" /></template>
        开始抓包
      </el-button>
    </div>
    <p class="net-hint">
      用隐藏 WebView 抓取页面里的媒体地址（m3u8/mp4/flv/mpd），不做 HTTPS 中间人。
    </p>

    <HintBar v-if="sniffError" variant="error">抓包失败：{{ sniffError }}</HintBar>
    <p v-if="sniffError" class="sniff-fail-hint">
      需要桌面端 WebView；如果该播放页/解析依赖 Android jar 源的 proxy，需 Android Host（后续支持）。
    </p>
    <div v-else-if="sniffResult" class="sniff-result">
      <div class="sniff-row">
        <span class="sniff-label">直链</span>
        <span class="sniff-value mono" :title="sniffResult.url">{{ sniffResult.url || '（空）' }}</span>
      </div>
      <div v-if="sniffedHeaders.length" class="sniff-row">
        <span class="sniff-label">请求头</span>
        <span class="sniff-value mono">{{ sniffedHeaders.join(' · ') }}</span>
      </div>
      <div v-if="sniffResult.parseName" class="sniff-row">
        <span class="sniff-label">解析器</span>
        <span class="sniff-value">{{ sniffResult.parseName }}</span>
      </div>
      <div v-if="sniffTrace.length" class="sniff-row sniff-trace">
        <span class="sniff-label">trace</span>
        <span class="sniff-value mono">{{ sniffTrace.join(' · ') }}</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.net-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}

.net-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
}

.net-spacer {
  flex: 1;
}

.net-empty {
  margin: 0;
  padding: var(--space-3);
  border: 1px dashed var(--border);
  border-radius: var(--radius-md);
  color: var(--text-faint);
  font-size: 12px;
  text-align: center;
}

.net-table {
  width: 100%;
}

.net-url,
.net-error {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.net-url {
  font-family: var(--font-mono);
  font-size: 11.5px;
}

.net-error {
  font-size: 11.5px;
  color: var(--danger);
}

.net-divider {
  margin: var(--space-4) 0 var(--space-3);
}

.sniff-form {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.sniff-input {
  flex: 1;
  min-width: 240px;
}

.net-hint {
  margin: var(--space-1) 0 0;
  font-size: 11px;
  color: var(--text-faint);
}

.sniff-fail-hint {
  margin: var(--space-1) 0 var(--space-3);
  padding-left: var(--space-3);
  border-left: 3px solid var(--warning);
  font-size: 11.5px;
  color: var(--text-muted);
  line-height: 1.6;
}

.sniff-result {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-top: var(--space-3);
  padding: var(--space-3);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.sniff-row {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: 0;
}

.sniff-label {
  flex: 0 0 48px;
  font-size: 12px;
  color: var(--text-muted);
}

.sniff-value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--text-regular);
}

.mono {
  font-family: var(--font-mono);
  font-size: 11.5px;
}

.sniff-trace .sniff-value {
  white-space: normal;
}
.sniff-form{
  padding-top: var(--space-4);
}
</style>
