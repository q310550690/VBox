<!-- 设置页 · Node 与 drpy：Node.js 解释器路径、版本、探测状态；drpy 规则依赖它 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { NodeStatus } from '@/types'

const ui = useUiStore()

/**
 * Windows 扩展长度路径前缀（`\\?\`）只对 Win32 API 有意义，展示给用户时还原成普通路径：
 * `\\?\D:\dir\node.exe` → `D:\dir\node.exe`；`\\?\UNC\server\share\x` → `\\server\share\x`。
 * 后端已在路径来源处规范化，这里再兜一层（老版本保存的 preferred 路径可能带前缀）。
 */
function prettyPath(value?: string | null): string {
  if (!value) return ''
  if (value.startsWith('\\\\?\\UNC\\')) return '\\\\' + value.slice(8)
  if (value.startsWith('\\\\?\\')) return value.slice(4)
  return value
}

const status = ref<NodeStatus | null>(null)
const pathInput = ref('')
const loading = ref(false)
const saving = ref(false)
const error = ref('')

const statusText = computed(() => {
  if (!status.value) return '未检测'
  if (status.value.available) {
    return `已就绪${status.value.version ? ` · ${status.value.version}` : ''}`
  }
  return '未找到可用 Node.js'
})

async function load() {
  if (api.demoMode) return
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const next = await api.nodeStatus()
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
    const next = await api.setNodePath(pathInput.value)
    status.value = next
    pathInput.value = prettyPath(next.preferred)
    ui.notify(
      next.available ? 'Node.js 已保存并可用' : '已保存，但未检测到可用 Node.js',
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
    const next = await api.refreshNodeStatus()
    status.value = next
    pathInput.value = prettyPath(next.preferred)
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-node">
    <header class="section-head">
      <h2 id="settings-node" class="section-title">Node 与 drpy</h2>
      <p class="section-desc">
        drpy/drpy2 规则由 Node.js 执行；正式安装包可内置一份 Node（随包分发），
        没有内置时会自动探测本机 Node.js 18+，也可手动指定路径。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：Node 检测不可用；桌面端（Tauri）才会真正探测本机 Node。
    </HintBar>

    <div class="node-status" :class="{ ok: status?.available, bad: status && !status.available }">
      <span class="status-dot" />
      <div class="status-text">
        <strong>{{ statusText }}</strong>
        <span v-if="status?.label" class="status-label">解释器：{{ prettyPath(status.label) }}</span>
        <span v-if="status?.bundled" class="status-label">随应用集成：{{ prettyPath(status.bundled) }}</span>
        <span v-if="status?.preferred" class="status-label">已保存路径：{{ prettyPath(status.preferred) }}</span>
        <span v-else-if="!status?.bundled" class="status-label">当前为自动探测（VBOX_NODE → node → node.exe）</span>
        <span v-if="status?.error" class="status-error">{{ status.error }}</span>
      </div>
      <el-button :loading="loading" @click="refresh">
        <template #icon><SolarIcon name="refresh" /></template>
        重新检测
      </el-button>
    </div>

    <div class="node-path">
      <el-input
        v-model="pathInput"
        clearable
        placeholder="留空自动探测；也可填写 C:\Program Files\nodejs\node.exe"
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

    <p class="node-tip">
      保存后写入 <code>app_data/node.json</code>；支持 drpy2 常见同步规则。
    </p>

    <HintBar v-if="error" variant="error">{{ error }}</HintBar>
  </section>
</template>

<style scoped>
.node-status {
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

.node-status.ok {
  border-color: var(--el-color-success);
}

.node-status.bad {
  border-color: var(--el-color-warning);
}

.node-status.ok .status-dot {
  background: var(--el-color-success);
}

.node-status.bad .status-dot {
  background: var(--el-color-warning);
}

.node-path {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: center;
}

.node-path :deep(.el-input) {
  flex: 1 1 320px;
}

.node-tip {
  margin: var(--space-2) 0 0;
  color: var(--el-text-color-secondary);
  font-size: 12px;
}
</style>
