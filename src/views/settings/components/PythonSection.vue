<!-- 设置页 · Python 运行时：解释器路径、探测状态、常用依赖检查、pip 安装命令 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { PythonStatus } from '@/types'

const ui = useUiStore()

/**
 * Windows 扩展长度路径前缀（`\\?\`）只对 Win32 API 有意义，展示给用户时还原成普通路径：
 * `\\?\D:\dir\python.exe` → `D:\dir\python.exe`；`\\?\UNC\server\share\x` → `\\server\share\x`。
 * 后端已在路径来源处规范化，这里再兜一层（老版本保存的 preferred 路径可能带前缀）。
 */
function prettyPath(value?: string | null): string {
  if (!value) return ''
  if (value.startsWith('\\\\?\\UNC\\')) return '\\\\' + value.slice(8)
  if (value.startsWith('\\\\?\\')) return value.slice(4)
  return value
}

const status = ref<PythonStatus | null>(null)
const pathInput = ref('')
const loading = ref(false)
const saving = ref(false)
const error = ref('')

const pipCommand = 'py -3 -m pip install requests beautifulsoup4 lxml pycryptodome pyquery'
/** 未安装 Python 时的 Windows 安装命令（winget） */
const installCommand =
  'winget install --id Python.Python.3.14 -e --accept-package-agreements --accept-source-agreements'

const dependencyLabel: Record<string, string> = {
  requests: 'requests',
  bs4: 'beautifulsoup4',
  lxml: 'lxml',
  Crypto: 'pycryptodome',
  pyquery: 'pyquery',
}

const availableDeps = computed(
  () => status.value?.dependencies.filter((dep) => dep.available).length ?? 0,
)
const totalDeps = computed(() => status.value?.dependencies.length ?? 0)
const statusText = computed(() => {
  if (!status.value) return '未检测'
  if (status.value.available) {
    return `已就绪${status.value.version ? ` · Python ${status.value.version}` : ''}`
  }
  return '未找到可用解释器'
})

async function load() {
  if (api.demoMode) return
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const next = await api.pythonStatus()
    status.value = next
    pathInput.value = prettyPath(next.preferred)
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

async function save() {
  if (saving.value) return
  saving.value = true
  error.value = ''
  try {
    const next = await api.setPythonPath(pathInput.value)
    status.value = next
    pathInput.value = prettyPath(next.preferred)
    ui.notify(
      next.available ? 'Python 解释器已保存并可用' : '已保存，但未检测到可用解释器',
      next.available ? 'success' : 'warning',
    )
  } catch (e) {
    error.value = api.formatError(e)
    ui.notify('保存失败：' + error.value, 'error')
  } finally {
    saving.value = false
  }
}

async function resetAuto() {
  pathInput.value = ''
  await save()
}

async function refresh() {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const next = await api.refreshPythonStatus()
    status.value = next
    pathInput.value = prettyPath(next.preferred)
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

async function copyPip() {
  try {
    await navigator.clipboard.writeText(pipCommand)
    ui.notify('pip 命令已复制', 'success')
  } catch {
    ui.notify('复制失败，请手动选择文本', 'warning')
  }
}

/** 复制 Windows 安装 Python 的 winget 命令 */
async function copyInstall() {
  try {
    await navigator.clipboard.writeText(installCommand)
    ui.notify('安装命令已复制', 'success')
  } catch {
    ui.notify('复制失败，请手动选择文本', 'warning')
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-python">
    <header class="section-head">
      <h2 id="settings-python" class="section-title">Python 运行时</h2>
      <p class="section-desc">
        VBox 的 <code>.py</code> spider 通过 CPython 执行。正式安装包可内置一份
        CPython + 常用依赖（随包分发）；没有内置时会自动探测本机解释器，也可手动指定。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：Python 检测不可用；桌面端（Tauri）才会真正探测本机解释器。
    </HintBar>

    <div class="python-status" :class="{ ok: status?.available, bad: status && !status.available }">
      <span class="status-dot" />
      <div class="status-text">
        <strong>{{ statusText }}</strong>
        <span v-if="status?.label" class="status-label">解释器：{{ prettyPath(status.label) }}</span>
        <span v-if="status?.bundled" class="status-label">随应用集成：{{ prettyPath(status.bundled) }}</span>
        <span v-if="status?.preferred" class="status-label">已保存路径：{{ prettyPath(status.preferred) }}</span>
        <span v-else-if="!status?.bundled" class="status-label">当前为自动探测（VBOX_PYTHON → 已知安装目录 / py launcher → python → python3）</span>
        <span v-if="status?.error" class="status-error">{{ status.error }}</span>
      </div>
      <el-button :loading="loading" @click="refresh">
        <template #icon><SolarIcon name="refresh" /></template>
        重新检测
      </el-button>
    </div>

    <div class="python-path">
      <el-input
        v-model="pathInput"
        clearable
        placeholder="留空自动探测；也可填写 py -3 或 C:\\Python312\\python.exe"
        @keyup.enter="save"
      />
      <el-button type="primary" :loading="saving" @click="save">
        <template #icon><SolarIcon name="check-circle" /></template>
        保存并检测
      </el-button>
      <el-button :disabled="saving" @click="resetAuto">
        <template #icon><SolarIcon name="restart" /></template>
        自动探测
      </el-button>
    </div>

    <p class="python-tip">
      支持绝对路径与参数命令（如 <code>py -3</code>）。
    </p>

    <!-- 未安装 Python 时的安装命令（Windows / winget） -->
    <div class="install-row">
      <span class="install-label">未安装 Python（Windows）可执行：</span>
      <code class="pip-command">{{ installCommand }}</code>
      <el-button @click="copyInstall">
        <template #icon><SolarIcon name="copy" /></template>
        复制
      </el-button>
    </div>

    <!-- 依赖 -->
    <div class="deps-head">
      <span class="deps-title">常用依赖</span>
      <span v-if="status" class="deps-count">{{ availableDeps }} / {{ totalDeps }} 已安装</span>
    </div>
    <div v-if="status?.dependencies.length" class="deps-list">
      <span
        v-for="dep in status.dependencies"
        :key="dep.name"
        class="dep-badge"
        :class="{ ok: dep.available }"
        :title="dep.module"
      >
        <SolarIcon v-if="dep.available" name="check-circle" />
        <SolarIcon v-else name="close" />
        {{ dependencyLabel[dep.name] ?? dep.name }}
      </span>
    </div>
    <p v-else-if="status && !status.dependencies.length" class="python-tip">
      未检测到依赖信息（解释器不可用或 Python 启动失败）。
    </p>

    <div class="pip-row">
      <code class="pip-command">{{ pipCommand }}</code>
      <el-button @click="copyPip">
        <template #icon><SolarIcon name="copy" /></template>
        复制
      </el-button>
    </div>

    <HintBar v-if="error" variant="error">{{ error }}</HintBar>
  </section>
</template>

<style scoped>
.python-status {
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

.python-status.ok {
  border-color: var(--el-color-success);
}

.python-status.bad {
  border-color: var(--el-color-warning);
}

.python-status.ok .status-dot {
  background: var(--el-color-success);
}

.python-status.bad .status-dot {
  background: var(--el-color-warning);
}

.python-path {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: center;
}

.python-path :deep(.el-input) {
  flex: 1 1 320px;
}

.python-tip {
  margin: 6px 0;
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.deps-head {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  margin: 14px 0 6px;
}

.deps-title {
  font-weight: 600;
}

.deps-count {
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.deps-list {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.dep-badge {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  padding: 2px var(--space-2);
  border: 0;
  background: var(--surface-2);
  border-radius: var(--radius-pill);
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.dep-badge.ok {
  color: var(--el-color-success);
  border-color: var(--el-color-success);
}

.pip-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: 14px;
}

.install-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: 10px;
}

.install-label {
  flex: none;
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.pip-command {
  display: flex;
  flex: 1;
  align-items: center;
  box-sizing: border-box;
  min-width: 0;
  height: 32px;
  padding: 0 var(--space-2);
  border-radius: 6px;
  background: var(--accent-weak);
  font-size: 12px;
  line-height: 1;
  overflow-x: auto;
  white-space: nowrap;
}
</style>
