<!-- 设置页 · JVM 车道：运行时状态、宿主池、dex 转换缓存清理、覆盖口径 -->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { JvmConvertSnapshot, JvmStatus, TelemetryStatus } from '@/types'

const ui = useUiStore()

const status = ref<JvmStatus | null>(null)
const convert = ref<JvmConvertSnapshot | null>(null)
const tel = ref<TelemetryStatus | null>(null)
const telBusy = ref(false)
const loading = ref(false)
const clearing = ref(false)
const error = ref('')

const modeText = computed(() => {
  switch (status.value?.mode) {
    case 'primary':
      return '主力车道（带 jar 的站点优先走 JRE）'
    case 'fallback':
      return '回退模式（VBOX_JVM_MODE=fallback，原生家族优先）'
    case 'off':
      return '已关闭（VBOX_JVM_MODE=off）'
    default:
      return '未检测'
  }
})

const statusText = computed(() => {
  if (!status.value) return '未检测'
  return status.value.available ? '运行时就绪' : '未找到 JRE / 兼容层（缺 fetch:jre 或 build:jvm 产物）'
})

const missing = computed(() => status.value?.missingClasses ?? [])
const pool = computed(() => status.value?.pool)
const coverage = computed(() => status.value?.coverage)

const converting = computed(() => (convert.value?.items ?? []).filter((it) => !it.done))
const recentDone = computed(() => (convert.value?.items ?? []).filter((it) => it.done).slice(0, 3))

function percent(item: { classesDone: number; totalClasses: number }): number {
  if (!item.totalClasses) return 0
  return Math.min(100, Math.round((item.classesDone / item.totalClasses) * 100))
}

function jarName(path: string): string {
  const parts = path.split(/[\\/]/)
  return parts[parts.length - 1] || path
}

async function loadConvertProgress() {
  if (api.demoMode) return
  try {
    convert.value = await api.jvmConvertProgress()
  } catch {
    // 进度是增强信息，失败不影响主状态
  }
}

async function load() {
  if (api.demoMode) return
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    status.value = await api.jvmStatus()
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
  try {
    tel.value = await api.telemetryStatus()
  } catch {
    // 遥测状态是增强信息，失败不影响主状态
  }
  await loadConvertProgress()
}

async function exportDiagnostics() {
  telBusy.value = true
  try {
    const path = await api.telemetryExport()
    ui.notify(`诊断包已导出：${path}`, 'success')
  } catch (e) {
    ui.notify('导出失败：' + api.formatError(e), 'error')
  } finally {
    telBusy.value = false
  }
}

async function openFeedbackIssue() {
  telBusy.value = true
  try {
    const url = await api.telemetryIssueUrl()
    await api.pluginOpenUrl(url)
  } catch (e) {
    ui.notify('打开反馈页失败：' + api.formatError(e), 'error')
  } finally {
    telBusy.value = false
  }
}

async function clearTelemetry() {
  telBusy.value = true
  try {
    const bytes = await api.telemetryClear()
    ui.notify(`已清空本机兼容性记录，释放 ${bytes} B`, 'success')
    await load()
  } catch (e) {
    ui.notify('清空失败：' + api.formatError(e), 'error')
  } finally {
    telBusy.value = false
  }
}

async function clearCache() {
  clearing.value = true
  error.value = ''
  try {
    const bytes = await api.jvmClearCache()
    ui.notify(`已清理转换缓存，释放 ${(bytes / 1024 / 1024).toFixed(1)} MB`, 'success')
    await load()
  } catch (e) {
    error.value = api.formatError(e)
    ui.notify('清理失败：' + error.value, 'error')
  } finally {
    clearing.value = false
  }
}

let progressTimer: number | undefined

onMounted(() => {
  void load()
  progressTimer = window.setInterval(() => void loadConvertProgress(), 2000)
})

onUnmounted(() => {
  if (progressTimer) window.clearInterval(progressTimer)
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-jvm">
    <header class="section-head">
      <h2 id="settings-jvm" class="section-title">JVM 车道（jar 蜘蛛）</h2>
      <p class="section-desc">
        type=3 的 jar 蜘蛛在进程外的真 JRE 17 上跑原始字节码：兼容层提供 Android/TVBox 的类，
        dex 样本按需转成 class 后缓存。没有可用运行时时会自动退回原生家族。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：JVM 状态不可用；桌面端（Tauri）才会真正探测运行环境。
    </HintBar>

    <div class="jvm-status" :class="{ ok: status?.available, bad: status && !status.available }">
      <span class="status-dot" />
      <div class="status-text">
        <strong>{{ statusText }}</strong>
        <span class="status-label">{{ modeText }}</span>
        <span v-if="status?.root" class="status-label">运行时：{{ status.root }}</span>
        <span v-if="status?.java" class="status-label">
          java：{{ status.java }}
          <template v-if="status?.javaSource">（来源 {{ status.javaSource }}）</template>
        </span>
        <span v-if="status?.cacheDir" class="status-label">转换缓存：{{ status.cacheDir }}</span>
      </div>
      <el-button :loading="loading" @click="load">
        <template #icon><SolarIcon name="refresh" /></template>
        刷新状态
      </el-button>
    </div>

    <div v-if="pool" class="jvm-grid">
      <div class="jvm-metric">
        <span class="metric-label">宿主池</span>
        <strong>{{ pool.hosts }} / {{ pool.maxHosts }}</strong>
        <span class="metric-sub">新建 {{ pool.spawns }} · 复用 {{ pool.hits }}</span>
      </div>
      <div class="jvm-metric">
        <span class="metric-label">调用</span>
        <strong>{{ pool.calls }}</strong>
        <span class="metric-sub">失败 {{ pool.failures }} · 回收 {{ pool.reaped }}</span>
      </div>
      <div class="jvm-metric">
        <span class="metric-label">播放会话</span>
        <strong>{{ pool.sessions }}</strong>
        <span class="metric-sub">宿主租约钉到播放器停止</span>
      </div>
      <div class="jvm-metric">
        <span class="metric-label">淘汰</span>
        <strong>{{ pool.evicted }}</strong>
        <span class="metric-sub">崩溃/换 ext 时摘除</span>
      </div>
    </div>

    <div v-if="converting.length || recentDone.length" class="jvm-convert">
      <p class="jvm-convert-title">
        dex → jar 转换<template v-if="converting.length">：<strong>{{ converting.length }}</strong> 个进行中</template>
      </p>
      <ul class="jvm-convert-list">
        <li v-for="it in converting" :key="it.key" class="jvm-convert-row">
          <div class="jvm-convert-head">
            <code>{{ jarName(it.jar) }}</code>
            <span class="jvm-convert-pct">
              {{ percent(it) }}%（{{ it.classesDone }} / {{ it.totalClasses || '?' }} 类）
            </span>
          </div>
          <el-progress :percentage="percent(it)" :show-text="false" :stroke-width="6" />
          <span class="jvm-convert-sub">
            {{ it.bytes ? (it.bytes / 1024 / 1024).toFixed(1) + ' MB' : '' }}
            · 已用时 {{ (it.elapsedMs / 1000).toFixed(1) }} s
          </span>
        </li>
        <li v-for="it in recentDone" :key="it.key + '-done'" class="jvm-convert-row done">
          <span class="jvm-convert-sub">
            {{ jarName(it.jar) }} ·
            <template v-if="it.ok">已完成 {{ it.classesDone }} 类（{{ (it.elapsedMs / 1000).toFixed(1) }} s）</template>
            <template v-else>失败：{{ it.error || '未知原因' }}</template>
          </span>
        </li>
      </ul>
    </div>

    <div v-if="coverage" class="jvm-coverage">
      <p>
        覆盖口径：带 jar 的 api <strong>{{ coverage.jarApis }}</strong> 个；
        能交给我们跑 <strong>{{ coverage.capableSites }}</strong> 个站点，
        加固壳 <strong>{{ coverage.guardedSites }}</strong> 个站点
        （共 {{ coverage.totalSites }} 个站点，剩余为 script/rulepack）。
      </p>
    </div>

    <div v-if="missing.length" class="jvm-missing">
      <p class="jvm-missing-title">缺类诊断：<strong>{{ missing.length }}</strong> 个类还没补（按命中次数）</p>
      <ul class="jvm-missing-list">
        <li v-for="m in missing.slice(0, 6)" :key="m.class">
          <code>{{ m.class }}</code><span class="jvm-missing-hits">×{{ m.hits }}</span>
        </li>
      </ul>
      <p class="jvm-tip">
        这些类来自蜘蛛实际调用（<code>ClassNotFoundException</code>）。补进
        <code>jvm/compat/src</code> 后重新 <code>node scripts/build-jvm.mjs</code> 即可，命中次数越多越值得先补。
      </p>
    </div>

    <div class="jvm-actions">
      <el-button :loading="clearing" @click="clearCache">
        <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
        清理 dex 转换缓存
      </el-button>
      <span class="jvm-tip">只删缓存与日志，不动加固壳载荷（<code>payloads/</code>）。</span>
    </div>

    <div v-if="tel" class="jvm-feedback">
      <p class="jvm-feedback-title">兼容性反馈（本机记录，<strong>不上传</strong>）</p>
      <p class="jvm-feedback-body">
        已记录 <strong>{{ tel.pendingJars }}</strong> 只新 jar 的结构画像、
        <strong>{{ tel.pendingFailures }}</strong> 条失败诊断。
        导出诊断包或生成预填 Issue 后可自行发送给我们，用来排适配优先级。
      </p>
      <div class="jvm-actions">
        <el-button :loading="telBusy" @click="exportDiagnostics">导出诊断包</el-button>
        <el-button :loading="telBusy" @click="openFeedbackIssue">生成反馈 Issue</el-button>
        <el-button :loading="telBusy" @click="clearTelemetry">清空记录</el-button>
      </div>
      <p class="jvm-tip">记录目录：<code>{{ tel.dir }}</code>（安装 ID {{ tel.installId }}）</p>
    </div>

    <p class="jvm-tip">
      加固壳站点会返回 <code>catvod.host.shell_unsupported</code>：这类 jar 的 init 要跑 ARM native 解密，
      需要离线工具或厂商提供解密后的载荷（放到 <code>payloads/&lt;jar sha256&gt;/index.json</code>）。
    </p>

    <HintBar v-if="error" variant="error">{{ error }}</HintBar>
  </section>
</template>

<style scoped>
.jvm-missing {
  margin: var(--space-3) 0;
  padding: 10px var(--space-3);
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.jvm-missing-title {
  margin: 0 0 6px;
  color: var(--el-text-color-primary);
  font-size: 13px;
}

.jvm-missing-list {
  margin: 0;
  padding-left: 18px;
  color: var(--el-text-color-regular);
  font-size: 12px;
  line-height: 1.9;
}

.jvm-missing-hits {
  margin-left: 6px;
  color: var(--el-text-color-secondary);
}

.jvm-status {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: 10px var(--space-3);
  margin: var(--space-3) 0;
  border: 0;
  border-left: 3px solid var(--text-faint);
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.jvm-status.ok {
  border-color: var(--el-color-success);
}

.jvm-status.bad {
  border-color: var(--el-color-warning);
}

.jvm-status.ok .status-dot {
  background: var(--el-color-success);
}

.jvm-status.bad .status-dot {
  background: var(--el-color-warning);
}

.jvm-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
  gap: var(--space-2);
  margin: var(--space-3) 0;
}

.jvm-metric {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px var(--space-3);
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.metric-label {
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.metric-sub {
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.jvm-coverage {
  padding: 10px var(--space-3);
  border-radius: var(--radius-md);
  background: var(--surface-inset);
  font-size: 13px;
}

.jvm-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: center;
  margin-top: var(--space-3);
}

.jvm-tip {
  margin: var(--space-2) 0 0;
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.status-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
}

.status-label {
  color: var(--el-text-color-secondary);
  font-size: 12px;
  word-break: break-all;
}

.status-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  border-radius: 50%;
  background: var(--text-faint);
}

.jvm-feedback {
  margin-top: 12px;
  padding: 12px 14px;
  border-radius: 10px;
  background: var(--vbox-surface-muted, rgba(127, 127, 127, 0.06));
}

.jvm-feedback-title {
  margin: 0 0 6px;
  font-size: 13px;
  font-weight: 600;
}

.jvm-feedback-body {
  margin: 0 0 10px;
  font-size: 13px;
  line-height: 1.6;
  opacity: 0.85;
}

.jvm-convert {
  margin-top: 12px;
  padding: 12px 14px;
  border-radius: 10px;
  background: var(--vbox-surface-muted, rgba(127, 127, 127, 0.06));
}

.jvm-convert-title {
  margin: 0 0 8px;
  font-size: 13px;
  opacity: 0.85;
}

.jvm-convert-list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.jvm-convert-head {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  font-size: 13px;
  margin-bottom: 4px;
}

.jvm-convert-pct {
  opacity: 0.75;
}

.jvm-convert-sub {
  font-size: 12px;
  opacity: 0.65;
}

</style>
