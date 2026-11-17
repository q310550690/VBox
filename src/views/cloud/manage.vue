<!-- 云盘管理（W14 §8.6）：只读浏览已登录网盘的目录。
     drive_list 已接入（W14 P2+）；未登录/不支持/凭据失效时显示后端错误提示。 -->
<script setup lang="ts">
import { computed, onActivated, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import PageScroll from '@/components/PageScroll.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import type { CloudDriveEntry } from '@/api'
import { useDownloadsStore } from '@/store/downloads'
import { formatBytes, formatRelativeTime } from '@/utils/format'

defineOptions({ name: 'CloudManageView' })

const PROVIDER_NAME: Record<string, string> = {
  p115: '115',
  quark: '夸克',
  xunlei: '迅雷',
  ali: '阿里',
  uc: 'UC',
  pikpak: 'PikPak',
}

const route = useRoute()
const router = useRouter()

const accountId = computed(() => String(route.params.accountId ?? ''))
const accounts = ref<Awaited<ReturnType<typeof api.panAccountsList>>>([])
const path = ref<{ id: string; name: string }[]>([])
const entries = ref<CloudDriveEntry[]>([])
const loading = ref(false)
const error = ref('')
const busyId = ref('')
const downloads = useDownloadsStore()

const account = computed(() => accounts.value.find((item) => item.account_id === accountId.value))
const providerName = computed(() => (account.value ? PROVIDER_NAME[account.value.provider] ?? account.value.provider : '云盘'))
const parentId = computed(() => path.value[path.value.length - 1]?.id ?? '')

async function loadAccounts(): Promise<void> {
  if (api.demoMode) return
  try {
    accounts.value = await api.panAccountsList()
  } catch {
    accounts.value = []
  }
}

async function loadDir(): Promise<void> {
  if (api.demoMode || !accountId.value) return
  loading.value = true
  error.value = ''
  try {
    entries.value = await api.driveList(accountId.value, parentId.value)
  } catch (err) {
    entries.value = []
    error.value = err instanceof Error ? err.message : String(err)
  } finally {
    loading.value = false
  }
}

function enter(entry: CloudDriveEntry): void {
  if (!entry.is_dir) return
  path.value = [...path.value, { id: entry.id, name: entry.name }]
  void loadDir()
}

function jumpTo(index: number): void {
  path.value = index < 0 ? [] : path.value.slice(0, index + 1)
  void loadDir()
}

function formatTime(ts?: number | null): string {
  if (!ts) return '—'
  return formatRelativeTime(ts > 1e12 ? ts : ts * 1000)
}

/** 解析文件直链（按钮 busy 态；失败把错误写到页面提示）。 */
async function resolveUrl(entry: CloudDriveEntry): Promise<string | null> {
  if (api.demoMode) return null
  busyId.value = entry.id
  try {
    return await api.driveDownload(accountId.value, entry.id)
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
    return null
  } finally {
    busyId.value = ''
  }
}

/** 播放：取临时直链交给内置播放器。 */
async function playFile(entry: CloudDriveEntry): Promise<void> {
  const url = await resolveUrl(entry)
  if (!url) return
  try {
    await api.playerOpen(url)
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
  }
}

/** 下载：优先交给下载中心；后端未就绪时回退浏览器打开直链。 */
async function downloadFile(entry: CloudDriveEntry): Promise<void> {
  const url = await resolveUrl(entry)
  if (!url) return
  try {
    await downloads.start({
      siteKey: `cloud:${accountId.value}`,
      siteName: `${providerName.value}云盘`,
      name: entry.name,
      epTitle: entry.name,
      url,
    })
  } catch {
    try {
      await api.pluginOpenUrl(url)
    } catch (err) {
      error.value = err instanceof Error ? err.message : String(err)
    }
  }
}

onMounted(() => {
  void loadAccounts()
  void loadDir()
})
onActivated(() => void loadDir())
</script>

<template>
  <PageScroll>
    <div class="cloud-manage-page">
      <header class="manage-top">
        <button
          class="back-btn"
          type="button"
          aria-label="返回云盘首页"
          @click="router.push('/cloud')"
        >
          <SolarIcon
            name="alt-arrow-left"
            :size="18"
          />
        </button>
        <h1 class="manage-title">
          我的云盘
        </h1>
        <span class="manage-provider">{{ providerName }}</span>
        <div class="manage-path">
          <button
            class="path-item"
            type="button"
            @click="jumpTo(-1)"
          >
            根目录
          </button>
          <template
            v-for="(item, index) in path"
            :key="item.id"
          >
            <span class="path-sep">/</span>
            <button
              class="path-item"
              type="button"
              @click="jumpTo(index)"
            >
              {{ item.name }}
            </button>
          </template>
        </div>
        <button
          class="refresh-btn"
          type="button"
          :disabled="loading"
          @click="loadDir"
        >
          ⟳
        </button>
      </header>

      <HintBar v-if="api.demoMode">
        浏览器演示模式：云盘管理只在桌面端（Tauri）可用。
      </HintBar>
      <HintBar
        v-if="error"
        variant="error"
      >
        {{ error }}
      </HintBar>

      <div
        v-if="loading"
        class="manage-loading"
      >
        正在读取目录…
      </div>

      <div
        v-else-if="entries.length"
        class="manage-table"
      >
        <div class="table-head">
          <span>名称</span><span>大小</span><span>修改时间</span><span>操作</span>
        </div>
        <div
          v-for="entry in entries"
          :key="entry.id"
          class="table-row"
        >
          <span class="entry-name">
            <SolarIcon
              :name="entry.is_dir ? 'folder-open' : 'play'"
              :size="16"
            />
            {{ entry.name }}
          </span>
          <span>{{ entry.is_dir ? '—' : formatBytes(entry.size ?? 0) }}</span>
          <span>{{ formatTime(entry.modified_at) }}</span>
          <span class="entry-actions">
            <button
              v-if="entry.is_dir"
              class="action-btn"
              type="button"
              @click="enter(entry)"
            >
              进入
            </button>
            <template v-else>
              <button
                class="action-btn"
                type="button"
                :disabled="busyId === entry.id"
                @click="playFile(entry)"
              >
                播放
              </button>
              <button
                class="action-btn"
                type="button"
                :disabled="busyId === entry.id"
                @click="downloadFile(entry)"
              >
                下载
              </button>
            </template>
          </span>
        </div>
      </div>

      <div
        v-else-if="!error"
        class="manage-empty"
      >
        目录为空
      </div>
    </div>
  </PageScroll>
</template>

<style scoped>
.cloud-manage-page {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-5) var(--space-6);
}

.manage-top {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}

.back-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text-muted);
  cursor: pointer;
}

.manage-title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: var(--text);
}

.manage-provider {
  padding: 2px 10px;
  border-radius: var(--radius-pill);
  background: var(--accent-weak);
  color: var(--accent);
  font-size: 12px;
}

.manage-path {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-muted);
}

.path-item {
  padding: 2px 4px;
  border: 0;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.path-item:hover {
  color: var(--accent);
}

.path-sep {
  color: var(--text-faint);
}

.refresh-btn {
  margin-left: auto;
  padding: 2px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
}

.manage-loading,
.manage-empty {
  padding: var(--space-5) 0;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}

.manage-table {
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  overflow: hidden;
}

.table-head,
.table-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 110px 150px 90px;
  gap: var(--space-2);
  align-items: center;
  padding: 8px var(--space-3);
  font-size: 13px;
}

.table-head {
  background: var(--surface-2);
  color: var(--text-muted);
  font-size: 12px;
}

.table-row {
  border-top: 1px solid var(--border-light);
  background: var(--surface);
  color: var(--text-regular);
}

.entry-name {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.entry-actions {
  display: flex;
  gap: var(--space-2);
}

.action-btn {
  padding: 2px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-regular);
  font-size: 12px;
  cursor: pointer;
}

.action-btn:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}

.action-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
</style>