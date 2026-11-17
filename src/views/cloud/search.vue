<!-- 云盘搜索结果页（W14 §8.3）
     KeepAlive include="CloudSearchView"：从详情浮层/其它页面返回时保留结果与滚动位置。 -->
<script setup lang="ts">
import { computed, onActivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import PageScroll from '@/components/PageScroll.vue'
import EmptyState from '@/components/EmptyState.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import type { CloudSearchItem, CloudSearchPage, CloudSearchProvider, CloudSortBy } from '@/api'
import { formatBytes, formatRelativeTime } from '@/utils/format'

defineOptions({ name: 'CloudSearchView' })

const route = useRoute()
const router = useRouter()

const PROVIDER_LABEL: Record<string, string> = {
  quark: '夸克',
  aliyun: '阿里',
  p115: '115',
  xunlei: '迅雷',
  pikpak: 'PikPak',
  uc: 'UC',
  baidu: '百度',
  tianyi: '天翼',
  mobile: '移动',
  p123: '123',
  magnet: '磁力',
  ed2k: '电驴',
  other: '其他',
}

const FILTERS: { value: CloudSearchProvider | ''; label: string }[] = [
  { value: '', label: '全部' },
  { value: 'quark', label: '夸克' },
  { value: 'aliyun', label: '阿里' },
  { value: 'p115', label: '115' },
  { value: 'xunlei', label: '迅雷' },
  { value: 'pikpak', label: 'PikPak' },
  { value: 'magnet', label: '磁力' },
]

const keyword = computed(() => String(route.params.keyword ?? ''))
const input = ref(keyword.value)
const provider = ref<CloudSearchProvider | ''>('')
const sort = ref<CloudSortBy>('relevance')
const page = ref(1)
const pageSize = 20

const result = ref<CloudSearchPage | null>(null)
const loading = ref(false)
const error = ref('')
const detail = ref<CloudSearchItem | null>(null)

/** 搜索源不可用：空结果且存在降级/超时来源（内置源依赖 Telegram，可能需要代理） */
const sourceTrouble = computed(() => {
  const value = result.value
  if (!value || value.items.length) return false
  return value.partial || value.degradedSources.length > 0
})

/** 已按当前筛选条件搜索过的关键词，避免返回本页时重复请求 */
let lastKey = ''

function filterLabel(providerKey: string): string {
  return PROVIDER_LABEL[providerKey] ?? providerKey
}

function formatTime(ts?: number | null): string {
  if (!ts) return ''
  // 后端统一按 Unix 秒；前端工具按毫秒
  return formatRelativeTime(ts > 1e12 ? ts : ts * 1000)
}

function titleOf(item: CloudSearchItem): string {
  return item.title || item.shareUrl
}

async function runSearch(force = false): Promise<void> {
  const key = `${keyword.value}|${provider.value}|${sort.value}`
  if (!force && key === lastKey && result.value) return
  lastKey = key
  loading.value = true
  error.value = ''
  try {
    if (api.demoMode) {
      result.value = { items: [], total: 0, partial: false, elapsedMs: 0, degradedSources: [] }
      return
    }
    result.value = await api.cloudSearch({
      keyword: keyword.value,
      page: page.value,
      pageSize,
      providers: provider.value ? [provider.value] : [],
      sort: sort.value,
    })
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
    result.value = null
  } finally {
    loading.value = false
  }
}

function submit(): void {
  const word = input.value.trim()
  if (!word) return
  page.value = 1
  if (word === keyword.value) {
    void runSearch(true)
    return
  }
  void router.push(`/cloud/search/${encodeURIComponent(word)}`)
}

function copyLink(item: CloudSearchItem): void {
  const text = item.password ? `${item.shareUrl} 提取码 ${item.password}` : item.shareUrl
  void navigator.clipboard
    .writeText(text)
    .then(() => ElMessage.success('链接已复制'))
    .catch(() => ElMessage.error('复制失败，请手动选择链接'))
}

async function openExternal(item: CloudSearchItem): Promise<void> {
  if (api.demoMode) return
  try {
    await api.pluginOpenUrl(item.shareUrl)
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : String(err))
  }
}

function play(item: CloudSearchItem): void {
  if (!item.playable) {
    ElMessage.info('该结果为磁力/电驴链接，暂不支持直接播放')
    return
  }
  // 播放链路（分享解析 → 直链 → 播放器）在后续里程碑接入；先打开分享页
  void openExternal(item)
}

watch(
  () => `${keyword.value}|${provider.value}|${sort.value}`,
  () => {
    input.value = keyword.value
    page.value = 1
    void runSearch()
  },
)

watch(page, () => void runSearch(true))

onMounted(() => void runSearch())
onActivated(() => void runSearch())
</script>

<template>
  <PageScroll>
    <div class="cloud-search-page">
      <header class="search-top">
        <button
          class="back-btn"
          type="button"
          aria-label="返回"
          @click="router.push('/cloud')"
        >
          <SolarIcon
            name="alt-arrow-left"
            :size="18"
          />
        </button>
        <div class="search-box">
          <input
            v-model="input"
            class="search-input"
            type="text"
            placeholder="搜索全网云盘资源…"
            @keydown.enter="submit"
          >
          <button
            class="search-submit"
            type="button"
            @click="submit"
          >
            搜索
          </button>
        </div>
        <select
          v-model="sort"
          class="search-select"
          aria-label="排序"
        >
          <option value="relevance">相关度</option>
          <option value="time">时间</option>
        </select>
      </header>

      <div
        v-if="result"
        class="search-stats"
      >
        共 {{ result.total }} 条
        <template v-if="result.partial">
          · <span class="stats-warn">部分来源超时，已显示 {{ result.items.length }} 条</span>
        </template>
        · 源 内置内核
        <template v-if="result.elapsedMs">
          · {{ (result.elapsedMs / 1000).toFixed(1) }}s
        </template>
        <button
          class="stats-refresh"
          type="button"
          @click="runSearch(true)"
        >
          ⟳ 刷新
        </button>
      </div>

      <div class="filter-chips">
        <button
          v-for="item in FILTERS"
          :key="item.value"
          class="filter-chip"
          :class="{ 'is-active': provider === item.value }"
          type="button"
          @click="provider = item.value"
        >
          {{ item.label }}
        </button>
      </div>

      <HintBar v-if="api.demoMode">
        浏览器演示模式：云盘聚合搜索只在桌面端（Tauri）可用。
      </HintBar>
      <HintBar
        v-if="error"
        variant="error"
      >
        {{ error }}
      </HintBar>

      <div
        v-if="sourceTrouble"
        class="source-trouble"
      >
        <span>
          搜索源暂时不可用：内置源依赖 Telegram 网络连通性，可能需要配置网络代理；也可以改用 PanSou 增强源。
        </span>
        <button
          class="stats-refresh"
          type="button"
          @click="router.push('/settings?section=anchor-cloudsearch')"
        >
          去配置
        </button>
      </div>

      <div
        v-if="loading"
        class="search-loading"
      >
        正在聚合来源…
      </div>

      <div
        v-else-if="result && result.items.length"
        class="result-list"
      >
        <article
          v-for="item in result.items"
          :key="item.id"
          class="result-row"
        >
          <div class="result-main">
            <h3 class="result-title">
              {{ titleOf(item) }}
            </h3>
            <div class="result-meta">
              <span class="provider-tag">{{ filterLabel(item.provider) }}</span>
              <span v-if="item.size">{{ formatBytes(item.size) }}</span>
              <span v-if="item.publishedAt">{{ formatTime(item.publishedAt) }}</span>
              <span class="result-sources">{{ item.sources.join(' · ') }}</span>
            </div>
            <div class="result-link">
              {{ item.shareUrl }}
              <span v-if="item.password">提取码 {{ item.password }}</span>
            </div>
          </div>
          <div class="result-actions">
            <button
              class="action-btn"
              type="button"
              :disabled="!item.playable"
              :title="item.playable ? '播放' : '暂不支持磁力/电驴播放'"
              @click="play(item)"
            >
              播放
            </button>
            <button
              class="action-btn"
              type="button"
              @click="copyLink(item)"
            >
              复制
            </button>
            <button
              class="action-btn"
              type="button"
              @click="detail = item"
            >
              详情
            </button>
          </div>
        </article>
      </div>

      <EmptyState
        v-else-if="result && !result.items.length"
        title="没有找到相关云盘资源"
        :desc="`没有找到与「${keyword}」相关的云盘资源，试试更短的关键词。`"
      />

      <el-dialog
        :model-value="!!detail"
        title="资源详情"
        width="560px"
        @update:model-value="detail = null"
      >
        <div
          v-if="detail"
          class="detail-grid"
        >
          <span class="detail-label">标题</span><span>{{ titleOf(detail) }}</span>
          <span class="detail-label">网盘</span><span>{{ filterLabel(detail.provider) }}</span>
          <span class="detail-label">大小</span><span>{{ detail.size ? formatBytes(detail.size) : '—' }}</span>
          <span class="detail-label">分享链接</span><span class="detail-link">{{ detail.shareUrl }}</span>
          <span class="detail-label">提取码</span><span>{{ detail.password || '—' }}</span>
          <span class="detail-label">来源</span><span>{{ detail.sources.join(' · ') || '—' }}</span>
          <span class="detail-label">时间</span><span>{{ formatTime(detail.publishedAt) || '—' }}</span>
        </div>
        <template #footer>
          <el-button @click="detail && copyLink(detail)">
            复制链接
          </el-button>
          <el-button @click="detail && openExternal(detail)">
            打开原页
          </el-button>
          <el-button
            type="primary"
            :disabled="!detail?.playable"
            @click="detail && play(detail)"
          >
            立即播放
          </el-button>
        </template>
      </el-dialog>
    </div>
  </PageScroll>
</template>

<style scoped>
.cloud-search-page {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-5) var(--space-6);
}

.search-top {
  display: flex;
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

.back-btn:hover {
  color: var(--text);
}

.search-box {
  display: flex;
  flex: 1;
  align-items: center;
  gap: var(--space-2);
  padding: 4px 4px 4px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
}

.search-box:focus-within {
  border-color: var(--accent);
}

.search-input {
  flex: 1;
  min-width: 0;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  line-height: 28px;
}

.search-submit {
  padding: 5px 16px;
  border: 0;
  border-radius: 6px;
  background: var(--accent);
  color: var(--text-on-accent);
  font-size: 13px;
  cursor: pointer;
}

.search-select {
  height: 32px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text);
  font-size: 13px;
}

.search-stats {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-muted);
}

.stats-warn {
  color: var(--warning);
}

.stats-refresh {
  margin-left: var(--space-2);
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.filter-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.source-trouble {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-3) var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--accent-weak);
  color: var(--text-muted);
  font-size: 13px;
  line-height: 1.6;
}

.filter-chip {
  padding: 4px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-pill);
  background: var(--surface);
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.filter-chip.is-active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-weak);
}

.search-loading {
  padding: var(--space-5) 0;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}

.result-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.result-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
}

.result-main {
  min-width: 0;
}

.result-title {
  margin: 0 0 4px;
  overflow: hidden;
  color: var(--text);
  font-size: 14px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  font-size: 12px;
  color: var(--text-muted);
}

.provider-tag {
  padding: 1px 8px;
  border-radius: var(--radius-pill);
  background: var(--accent-weak);
  color: var(--accent);
}

.result-sources {
  overflow: hidden;
  max-width: 260px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result-link {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: 4px;
  font-size: 12px;
  color: var(--text-faint);
}

.result-actions {
  display: flex;
  flex: 0 0 auto;
  gap: var(--space-2);
}

.action-btn {
  padding: 4px 12px;
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

.detail-grid {
  display: grid;
  grid-template-columns: 80px 1fr;
  gap: var(--space-2) var(--space-3);
  font-size: 13px;
  color: var(--text);
}

.detail-label {
  color: var(--text-muted);
}

.detail-link {
  word-break: break-all;
}
</style>