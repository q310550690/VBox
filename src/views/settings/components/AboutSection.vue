<!-- 设置页 · 关于分区：版本 / 技术栈 / 自动更新 / 不内置任何源 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import { formatError } from '@/api'
import { useConfigStore } from '@/store/config'
import {
  appVersion,
  checkUpdate,
  installUpdate,
  updatesSupported,
  type UpdateInfo,
} from '@/api/update'

const config = useConfigStore()

/** 当前版本：桌面端读 Tauri，浏览器预览用构建期常量兜底 */
const version = ref('—')
/** 更新状态机：idle / checking / latest / available / downloading / ready */
const status = ref<'idle' | 'checking' | 'latest' | 'available' | 'downloading' | 'ready'>('idle')
const checking = computed(() => status.value === 'checking')
const downloading = computed(() => status.value === 'downloading')
const updateInfo = ref<UpdateInfo | null>(null)
const percent = ref(0)
const error = ref('')

const statusText = computed(() => {
  if (!updatesSupported) return '浏览器预览模式不支持自动更新（需桌面端）'
  switch (status.value) {
    case 'checking':
      return '正在检查更新…'
    case 'latest':
      return `已是最新版本（v${version.value}）`
    case 'available':
      return `发现新版本 v${updateInfo.value?.version ?? ''}`
    case 'downloading':
      return `正在下载更新 ${percent.value}%`
    case 'ready':
      return '下载完成，正在重启应用…'
    default:
      return '检查更新以获取新版本；更新包会校验签名后再安装。'
  }
})

onMounted(async () => {
  try {
    version.value = await appVersion()
  } catch {
    version.value = __APP_VERSION__
  }
})

/** 检查更新（手动触发；启动时的静默检查在 App.vue） */
async function onCheck() {
  if (checking.value || downloading.value) return
  error.value = ''
  updateInfo.value = null
  percent.value = 0
  status.value = 'checking'
  try {
    const info = await checkUpdate()
    if (info) {
      updateInfo.value = info
      status.value = 'available'
    } else {
      status.value = 'latest'
    }
  } catch (e) {
    status.value = 'idle'
    error.value = `检查失败：${formatError(e)}`
  }
}

/** 下载并安装更新；成功后应用会自动重启，不需要手动操作 */
async function onInstall() {
  if (!updateInfo.value || downloading.value) return
  error.value = ''
  percent.value = 0
  status.value = 'downloading'
  try {
    await installUpdate((progress) => {
      if (progress.total && progress.total > 0) {
        percent.value = Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
      }
    })
    status.value = 'ready'
  } catch (e) {
    status.value = 'available'
    error.value = `更新失败：${formatError(e)}`
  }
}
</script>

<template>
  <section class="settings-section panel" aria-labelledby="settings-about">
    <header class="section-head">
      <h2 id="settings-about" class="section-title">关于</h2>
      <p class="section-desc">版本、运行环境与自动更新。</p>
    </header>

    <dl class="about-list">
      <div class="about-row">
        <dt class="about-key">应用</dt>
        <dd class="about-value">VBox {{ version }}</dd>
      </div>
      <div class="about-row">
        <dt class="about-key">技术栈</dt>
        <dd class="about-value">Vue 3 + Element Plus + Tauri 2</dd>
      </div>
      <div class="about-row">
        <dt class="about-key">运行模式</dt>
        <dd class="about-value">
          {{ config.demoMode ? '浏览器演示模式（无 Tauri IPC）' : 'Tauri 桌面端' }}
        </dd>
      </div>
    </dl>

    <div class="update-box">
      <div class="update-head">
        <div class="update-text">
          <p class="update-title">自动更新</p>
          <p class="update-desc">
            {{ statusText }}
          </p>
        </div>
        <el-button
          class="update-check"
          :loading="checking"
          :disabled="!updatesSupported || downloading"
          @click="onCheck"
        >
          <template #icon>
            <SolarIcon name="refresh" />
          </template>
          检查更新
        </el-button>
      </div>

      <el-progress
        v-if="downloading || status === 'ready'"
        class="update-progress"
        :percentage="percent"
        :stroke-width="6"
      />

      <div v-if="updateInfo && !downloading" class="update-available">
        <p class="update-available-title">新版本 v{{ updateInfo.version }}</p>
        <pre v-if="updateInfo.notes" class="update-notes">{{ updateInfo.notes }}</pre>
        <el-button
          type="primary"
          :disabled="!updatesSupported"
          @click="onInstall"
        >
          <template #icon>
            <SolarIcon name="download" />
          </template>
          下载并安装更新（完成后自动重启）
        </el-button>
      </div>

      <p v-if="error" class="update-error">
        <SolarIcon name="danger-triangle" :size="14" aria-hidden="true" />
        {{ error }}
      </p>
    </div>

    <p class="about-note">
      <SolarIcon class="about-note-icon" name="info-circle" aria-hidden="true" />
      VBox 不内置任何源：所有影视 / 直播内容均来自你自行添加的订阅源；请确保订阅源合法合规。
    </p>

    <HintBar v-if="config.demoMode">
      当前为浏览器演示模式：页面数据均来自 mock，不会读写本地配置与文件。
    </HintBar>
  </section>
</template>

<style scoped>
.about-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin: 0;
}

.about-row {
  display: flex;
  gap: var(--space-2);
  font-size: 13px;
}

.about-key {
  flex: 0 0 72px;
  color: var(--text-muted);
}

.about-value {
  margin: 0;
  color: var(--text);
}

.update-box {
  margin-top: var(--space-4);
  padding: var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.update-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-3);
}

.update-text {
  min-width: 0;
}

.update-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.update-desc {
  margin: var(--space-1) 0 0;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
}

.update-check {
  flex-shrink: 0;
}

.update-progress {
  margin-top: var(--space-3);
}

.update-available {
  margin-top: var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  align-items: flex-start;
}

.update-available-title {
  margin: 0;
  font-size: 13px;
  color: var(--text);
}

.update-notes {
  margin: 0;
  max-height: 160px;
  overflow: auto;
  padding: var(--space-2);
  width: 100%;
  box-sizing: border-box;
  border-radius: var(--radius-sm);
  background: var(--surface-3);
  color: var(--text-muted);
  font-family: inherit;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
}

.update-error {
  display: flex;
  align-items: flex-start;
  gap: var(--space-1);
  margin: var(--space-2) 0 0;
  font-size: 12px;
  color: var(--danger);
}

.about-note {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  margin: var(--space-4) 0 0;
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}

.about-note + .hint-bar.is-info {
  margin-top: var(--space-4);
}

.about-note-icon {
  flex-shrink: 0;
  margin-top: 2px;
  color: var(--accent);
}
</style>
