<!-- 设置页 · FFmpeg：随应用集成 / 一键下载 / 手动指定；SAMPLE-AES/分离音轨/DASH 兜底保存依赖它 -->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { FfmpegProvisionProgress, FfmpegStatus } from '@/types'

const ui = useUiStore()

/**
 * Windows 扩展长度路径前缀（`\\?\`）只对 Win32 API 有意义，展示给用户时还原成普通路径：
 * `\\?\D:\dir\ffmpeg.exe` → `D:\dir\ffmpeg.exe`；`\\?\UNC\server\share\x` → `\\server\share\x`。
 * 后端已在路径来源处规范化，这里再兜一层（老版本保存的 path 可能带前缀）。
 */
function prettyPath(value?: string | null): string {
  if (!value) return ''
  if (value.startsWith('\\\\?\\UNC\\')) return '\\\\' + value.slice(8)
  if (value.startsWith('\\\\?\\')) return value.slice(4)
  return value
}

const status = ref<FfmpegStatus | null>(null)
const pathInput = ref('')
const loading = ref(false)
const saving = ref(false)
const picking = ref(false)
const provisioning = ref(false)
const progress = ref<FfmpegProvisionProgress | null>(null)
const error = ref('')

/** 订阅下载进度（设置页卸载时取消） */
let unlistenProgress: (() => void) | null = null

/** 状态文案（与 Node 分区保持一致） */
const statusText = computed(() => {
  if (!status.value) return '未检测'
  if (status.value.available) {
    return `已就绪${status.value.version ? ` · ${status.value.version}` : ''}`
  }
  return '未找到可用 FFmpeg'
})

/** 来源说明 */
const sourceText = computed(() => {
  switch (status.value?.source) {
    case 'configured':
      return '设置页指定'
    case 'managed':
      return '已下载集成到本机'
    case 'bundled':
      return '随应用集成（开箱即用）'
    case 'env':
      return '环境变量 VBOX_FFMPEG'
    case 'path':
      return '系统 PATH'
    default:
      return status.value?.source || '—'
  }
})

/** 下载/解压进度百分比 */
const percent = computed(() => {
  const value = progress.value
  if (!value) return 0
  if (value.phase === 'done') return 100
  if (value.phase === 'verify' || value.phase === 'extract') return 99
  if (!value.total || value.total <= 0) return 0
  return Math.min(99, Math.round((value.downloaded / value.total) * 100))
})

async function load() {
  if (api.demoMode || loading.value) return
  loading.value = true
  error.value = ''
  try {
    const next = await api.ffmpegStatus()
    status.value = next
    pathInput.value = prettyPath(next.path)
    provisioning.value = next.provisioning
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

async function save() {
  if (saving.value || api.demoMode) {
    if (api.demoMode) ui.notify('浏览器预览模式不支持修改 FFmpeg 路径', 'info')
    return
  }
  saving.value = true
  error.value = ''
  try {
    const next = await api.ffmpegSetPath(pathInput.value.trim() || null)
    status.value = next
    pathInput.value = prettyPath(next.path)
    ui.notify(
      next.available ? 'FFmpeg 已保存并可用' : `已保存，但未检测到可用 FFmpeg：${next.error || '未知原因'}`,
      next.available ? 'success' : 'warning',
    )
  } catch (e) {
    error.value = api.formatError(e)
    ui.notify('保存失败：' + error.value, 'error')
  } finally {
    saving.value = false
  }
}

/** 恢复自动（已下载集成 → 随包集成 → VBOX_FFMPEG → PATH） */
async function resetAuto() {
  pathInput.value = ''
  await save()
}

/** 原生选择 ffmpeg 可执行文件 */
async function pick() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式不支持选择本地文件', 'info')
    return
  }
  if (picking.value) return
  picking.value = true
  try {
    const picked = await api.ffmpegPick()
    if (!picked) return
    pathInput.value = picked
    await save()
  } catch (e) {
    error.value = api.formatError(e)
    ui.notify('选择文件失败：' + error.value, 'error')
  } finally {
    picking.value = false
  }
}

/** 一键下载并集成：不需要用户自己装 FFmpeg */
async function provision() {
  if (api.demoMode) {
    ui.notify('浏览器预览模式无法下载 FFmpeg（需桌面端）', 'info')
    return
  }
  if (provisioning.value) return
  provisioning.value = true
  error.value = ''
  progress.value = null
  try {
    const next = await api.ffmpegProvision()
    status.value = next
    pathInput.value = prettyPath(next.path)
    ui.notify(
      next.available ? 'FFmpeg 已集成，无需再手动安装' : `下载完成但仍不可用：${next.error || '未知原因'}`,
      next.available ? 'success' : 'warning',
    )
  } catch (e) {
    error.value = api.formatError(e)
    ui.notify('集成失败：' + error.value, 'error')
  } finally {
    provisioning.value = false
  }
}

async function refresh() {
  if (loading.value || api.demoMode) return
  loading.value = true
  error.value = ''
  try {
    const next = await api.ffmpegStatus()
    status.value = next
    pathInput.value = prettyPath(next.path)
    provisioning.value = next.provisioning
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

/** 打开官方下载页 */
async function openDownload() {
  try {
    await api.ffmpegOpenDownload()
  } catch (e) {
    ui.notify('打开下载页失败：' + api.formatError(e), 'error')
  }
}

onMounted(async () => {
  unlistenProgress = await api.onFfmpegProvision((payload) => {
    progress.value = payload
    if (payload.phase === 'error') {
      error.value = payload.message
      provisioning.value = false
    } else if (payload.phase === 'done') {
      provisioning.value = false
    }
  })
  await load()
})

onUnmounted(() => {
  unlistenProgress?.()
  unlistenProgress = null
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-ffmpeg">
    <header class="section-head">
      <h2 id="settings-ffmpeg" class="section-title">FFmpeg</h2>
      <p class="section-desc">
        SAMPLE-AES 解密、分离音轨合并、DASH / MP4 兜底保存依赖 FFmpeg；普通播放与明文、AES-128 HLS 不需要。
        VBox 已尽量「自带」：正式安装包内置一份，缺失时可在下方一键下载集成，无需自己安装。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：FFmpeg 检测不可用；桌面端（Tauri）才会真正探测本机 FFmpeg。
    </HintBar>

    <div
      class="tool-status"
      :class="{ ok: status?.available, bad: status && !status.available }"
    >
      <span class="status-dot" />
      <div class="status-text">
        <strong>{{ statusText }}</strong>
        <span v-if="status?.effective" class="status-label">实际使用：{{ prettyPath(status.effective) }}</span>
        <span class="status-label">来源：{{ sourceText }}</span>
        <span v-if="status?.bundled" class="status-label">随应用集成：{{ prettyPath(status.bundled) }}</span>
        <span v-if="status?.managed" class="status-label">已下载集成：{{ prettyPath(status.managed) }}</span>
        <span v-if="status?.path" class="status-label">已保存路径：{{ prettyPath(status.path) }}</span>
        <span
          v-else-if="!status?.bundled && !status?.managed"
          class="status-label"
        >当前为自动探测（VBOX_FFMPEG → ffmpeg）</span>
        <span v-if="status?.error" class="status-error">{{ status.error }}</span>
      </div>
      <div class="status-actions">
        <el-button
          v-if="status && !status.available && status.canProvision"
          type="primary"
          :loading="provisioning"
          :disabled="api.demoMode"
          @click="provision"
        >
          <template #icon><SolarIcon name="download" /></template>
          {{ provisioning ? '正在集成…' : '一键下载并集成' }}
        </el-button>
        <el-button :loading="loading" :disabled="api.demoMode" @click="refresh">
          <template #icon><SolarIcon name="refresh" /></template>
          重新检测
        </el-button>
      </div>
    </div>

    <div v-if="provisioning || progress" class="provision-box">
      <el-progress
        :percentage="percent"
        :status="progress?.phase === 'error' ? 'exception' : undefined"
        :stroke-width="8"
      />
      <p class="provision-text">{{ progress?.message || '准备下载…' }}</p>
    </div>

    <div class="tool-path">
      <el-input
        v-model="pathInput"
        clearable
        :disabled="api.demoMode"
        placeholder="留空自动；也可填写 C:\ffmpeg\bin\ffmpeg.exe"
        @keyup.enter="save"
      />
      <el-button type="primary" :loading="saving" :disabled="api.demoMode" @click="save">
        <template #icon><SolarIcon name="check-circle" /></template>
        保存并检测
      </el-button>
      <el-button :disabled="saving || api.demoMode" @click="resetAuto">
        <template #icon><SolarIcon name="restart" /></template>
        自动
      </el-button>
      <el-button :loading="picking" :disabled="api.demoMode" @click="pick">
        <template #icon><SolarIcon name="folder-open" /></template>
        选择文件…
      </el-button>
    </div>

    <div v-if="status && !status.available" class="install-guide">
      <p class="guide-title">补齐 FFmpeg（任选一种）：</p>
      <ul class="guide-list">
        <li>
          点上方<strong>「一键下载并集成」</strong>：VBox 自动下载固定版本并做 SHA-256 校验，
          只需一次（约 112 MB）
        </li>
        <li>
          安装包自带的版本不可用/被删了：重新运行安装包，或执行
          <code>npm run fetch:ffmpeg</code> 后重新打包
        </li>
        <li>
          已有自己的 ffmpeg：Windows 可用 <code>winget install Gyan.FFmpeg</code> /
          <code>scoop install ffmpeg</code> / <code>choco install ffmpeg</code>，
          macOS <code>brew install ffmpeg</code>；然后用「选择文件…」指过去
        </li>
        <li>
          手动下载：
          <el-button link type="primary" @click="openDownload">
            <template #icon><SolarIcon name="square-arrow-right-up" /></template>
            ffmpeg.org/download
          </el-button>
          （或 gyan.dev / BtbN 构建），解压后选择 <code>bin\ffmpeg.exe</code>
        </li>
      </ul>
    </div>

    <HintBar v-if="error" variant="error">{{ error }}</HintBar>
  </section>
</template>

<style scoped>
.tool-status {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: 10px var(--space-3);
  margin: var(--space-3) 0;
  border: 1px solid var(--el-border-color);
  border-radius: var(--radius-md);
  background: var(--el-fill-color-lighter);
}

.tool-status.ok {
  border-color: var(--el-color-success-light-5);
}

.tool-status.bad {
  border-color: var(--el-color-warning-light-5);
}

.tool-status.ok .status-dot {
  background: var(--el-color-success);
}

.tool-status.bad .status-dot {
  background: var(--el-color-warning);
}

.status-actions {
  display: flex;
  gap: var(--space-2);
  flex: none;
}

.provision-box {
  margin: var(--space-2) 0 var(--space-3);
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--radius-md);
  background: var(--el-fill-color-blank);
}

.provision-text {
  margin: var(--space-2) 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

.tool-path {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: center;
}

.tool-path :deep(.el-input) {
  flex: 1 1 320px;
}

.install-guide {
  margin-top: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-left: 3px solid var(--el-color-warning);
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, var(--el-color-warning) 14%, transparent);
}

.guide-title {
  margin: 0 0 var(--space-1);
  font-size: 12.5px;
  font-weight: 600;
  color: var(--text-regular);
}

.guide-list {
  margin: 0;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.8;
  color: var(--text-muted);
}

.guide-list code {
  padding: 1px var(--space-1);
  border-radius: var(--radius-sm);
  background: var(--surface-2);
  font-size: 11.5px;
}
</style>
