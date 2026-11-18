<!-- 云盘搜索结果页（W14 §8.3）
     KeepAlive include="CloudSearchView"：从详情浮层/其它页面返回时保留结果与滚动位置。 -->
<script setup lang="ts">
import { computed, onActivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { storeToRefs } from 'pinia'
import SolarIcon from '@/components/SolarIcon.vue'
import PageScroll from '@/components/PageScroll.vue'
import EmptyState from '@/components/EmptyState.vue'
import SearchInput from '@/views/search/components/SearchInput.vue'
import { cloudSearchHint, detectCloudSearchCode } from '@/utils/cloudErrors'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import type { CloudSearchItem, CloudSearchProvider, CloudSortBy } from '@/api'
import { useCloudSearchStore } from '@/store/cloudSearch'
import { useCloudSourcesStore } from '@/store/cloudSources'
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

const CORE_FILTERS: CloudSearchProvider[] = [
  'quark',
  'aliyun',
  'p115',
  'xunlei',
  'pikpak',
  'uc',
  'magnet',
  'ed2k',
]
const CORE_FILTER_SET = new Set<string>(CORE_FILTERS)

const keyword = computed(() => String(route.params.keyword ?? ''))
const input = ref(keyword.value)
const detail = ref<CloudSearchItem | null>(null)

// 状态/结果缓存统一放 store（W14 §7.3）：KeepAlive 返回本页不重复请求后端。
const search = useCloudSearchStore()
const { provider, sort, page, pageSize, result, loading, error } = storeToRefs(search)
const pageCount = computed(() => search.pageCount)
const countsByProvider = computed(() => search.countsByProvider)
const allCount = computed(() => search.allCount)
const sourceTrouble = computed(() => search.sourceTrouble)

/** W14 §6.8：按后端错误码给动作提示（去配置 / 重试） */
const errorHint = computed(() => cloudSearchHint(detectCloudSearchCode(error.value ?? '')))

function chipCount(value: CloudSearchProvider | ''): number {
  if (value === '') return allCount.value
  return countsByProvider.value.get(value) ?? 0
}

/** 过滤标签：核心网盘常驻，其余 provider 仅在结果中出现时追加（W14 §8.3 动态 facet） */
const filters = computed<{ value: CloudSearchProvider | ''; label: string }[]>(() => {
  const present = new Set((result.value?.providerCounts ?? []).map((row) => row.provider))
  const extras = (Object.keys(PROVIDER_LABEL) as CloudSearchProvider[]).filter(
    (key) => !CORE_FILTER_SET.has(key) && present.has(key),
  )
  const active = provider.value && !CORE_FILTER_SET.has(provider.value) ? [provider.value] : []
  const values = [...new Set([...CORE_FILTERS, ...extras, ...active])]
  return [{ value: '', label: '全部' }, ...values.map((value) => ({ value, label: PROVIDER_LABEL[value] ?? value }))]
})

const sources = useCloudSourcesStore()
const sourceLabel = computed(() => {
  if (sources.priority === 'pansou_only') return 'PanSou 增强源'
  if (sources.priority === 'builtin_only') return '内置内核'
  return sources.pansouConfigured ? '内置内核 + PanSou' : '内置内核'
})

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

function runSearch(force = false): void {
  void search.run(force)
}

function goPage(target: number): void {
  search.goPage(target)
}

function setFilter(value: CloudSearchProvider | ''): void {
  search.setFilter(value)
}

function toggleSort(): void {
  const next: CloudSortBy = sort.value === 'relevance' ? 'time' : 'relevance'
  search.setSort(next)
}

function onPageSizeChange(event: Event): void {
  search.setPageSize(Number((event.target as HTMLSelectElement).value))
}

/** 分享被取消/过期的错误文案：标记失效，避免重复点击 */
const DEAD_SHARE_PATTERN = /失效|已被取消|取消分享|已取消|不存在|已过期/

/** 需要登录/凭据失效：引导到云盘首页账号管理抽屉（W14 §8.8） */
const LOGIN_REQUIRED_PATTERN = /需要登录|未登录|登录后重试|账号管理|重新登录/

function isDead(item: CloudSearchItem): boolean {
  return search.deadLinkSet.has(item.shareUrl)
}

/** 详情浮层「状态」：本地只标记过失效，未失效不代表已验证可解析 */
function statusOf(item: CloudSearchItem): string {
  return isDead(item) ? '分享失效' : '可解析（未验证）'
}

/** 转存按 provider 灰度：目前没有 provider 通过风控评估，按钮统一置灰「暂不支持」 */
const TRANSFER_ENABLED: ReadonlySet<string> = new Set<string>()

function transferAllowed(providerKey: string): boolean {
  return TRANSFER_ENABLED.has(providerKey)
}

function submit(): void {
  const word = input.value.trim()
  if (!word) return
  search.page = 1
  if (word === keyword.value) {
    void search.run(true)
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

function copyPassword(item: CloudSearchItem): void {
  if (!item.password) return
  void navigator.clipboard
    .writeText(item.password)
    .then(() => ElMessage.success('提取码已复制'))
    .catch(() => ElMessage.error('复制失败，请手动输入'))
}

async function openExternal(item: CloudSearchItem): Promise<void> {
  if (api.demoMode) return
  try {
    await api.pluginOpenUrl(item.shareUrl)
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : String(err))
  }
}

const playingId = ref('')

async function play(item: CloudSearchItem): Promise<void> {
  if (!item.playable) {
    ElMessage.info('该结果为磁力/电驴链接，暂不支持直接播放')
    return
  }
  if (playingId.value) return
  if (api.demoMode) {
    ElMessage.info('演示环境不执行播放')
    return
  }
  playingId.value = item.id
  try {
    const resolved = await api.cloudResolvePlay(item.shareUrl, item.password)
    if (resolved?.url) {
      search.unmarkDead(item.shareUrl)
      await api.playerOpen(resolved.url, resolved.headers)
      return
    }
  } catch (err) {
    // 未登录 / 夸克·UC 需转存等可操作错误：提示后回退打开原分享页
    const message = api.formatError(err)
    if (DEAD_SHARE_PATTERN.test(message)) {
      search.markDead(item.shareUrl)
      ElMessage.warning(`${message}（已标记为分享失效）`)
    } else if (LOGIN_REQUIRED_PATTERN.test(message)) {
      // 未登录播放：直接给「去登录」入口，跳到云盘首页并打开账号管理抽屉
      void ElMessageBox.confirm(message, '需要登录网盘账号', {
        confirmButtonText: '去登录',
        cancelButtonText: '稍后',
        type: 'warning',
      })
        .then(() => router.push({ path: '/cloud', query: { accounts: '1' } }))
        .catch(() => undefined)
    } else {
      ElMessage.warning(message)
    }
  } finally {
    playingId.value = ''
  }
  if (isDead(item)) return
  await openExternal(item)
}

watch(keyword, (word) => {
  input.value = word
  search.initialize(word)
  void search.run()
})

onMounted(() => {
  search.initialize(keyword.value)
  void search.run()
  void sources.load()
})
onActivated(() => {
  search.initialize(keyword.value)
  void search.run()
})
</script>

<template>
  <PageScroll>
    <div class="page cloud-search-page">
      <header class="search-top">
        <div class="search-left">
          <section class="search-bar panel">
            <el-button
              circle
              class="back-btn"
              aria-label="返回云盘"
              @click="router.push('/cloud')"
            >
              <template #icon>
                <SolarIcon name="arrow-left" />
              </template>
            </el-button>
            <SearchInput
              :model-value="input"
              :loading="loading"
              placeholder="搜索全网云盘资源…"
              @update:model-value="input = $event"
              @search="submit"
              @clear="input = ''"
            />
          </section>
      <div class="filter-chips">
        <button
          v-for="item in filters"
          :key="item.value"
          class="filter-chip"
          :class="{ 'is-active': provider === item.value }"
          type="button"
          @click="setFilter(item.value)"
        >
          {{ item.label }}
          <span
            v-if="chipCount(item.value)"
            class="filter-count"
          >{{ chipCount(item.value) }}</span>
        </button>
      </div>
        </div>
      </header>

      <div
        v-if="result"
        class="search-stats"
      >
        共 {{ result.total }} 条
        <template v-if="result.deduped">
          · 去重 {{ result.deduped }}
        </template>
        <template v-if="result.partial">
          · <span class="stats-warn">部分来源超时，已显示 {{ result.items.length }} 条</span>
        </template>
        · 源 {{ sourceLabel }}
        <template v-if="result.elapsedMs">
          · {{ (result.elapsedMs / 1000).toFixed(1) }}s
        </template>
        <button
          class="stats-sort-btn"
          type="button"
          :title="sort === 'relevance' ? '排序：相关度（点击切换为时间）' : '排序：时间（点击切换为相关度）'"
          :aria-label="sort === 'relevance' ? '排序：相关度' : '排序：时间'"
          @click="toggleSort"
        >
          <SolarIcon :name="sort === 'relevance' ? 'star' : 'clock-circle'" :size="14" />
          <span>{{ sort === 'relevance' ? '相关度' : '时间' }}</span>
        </button>
      </div>

      <HintBar v-if="api.demoMode">
        浏览器演示模式：云盘聚合搜索只在桌面端（Tauri）可用。
      </HintBar>
      <HintBar
        v-if="error"
        variant="error"
      >
        <div class="search-error">
          <span>{{ errorHint.text || error }}</span>
          <span
            v-if="errorHint.text"
            class="search-error-detail"
          >{{ error }}</span>
          <button
            v-if="errorHint.action === 'settings'"
            class="stats-refresh"
            type="button"
            @click="router.push('/settings?section=anchor-cloudsearch')"
          >
            去配置
          </button>
          <button
            v-else-if="errorHint.action === 'retry'"
            class="stats-refresh"
            type="button"
            @click="runSearch(true)"
          >
            重试
          </button>
        </div>
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
          @click="runSearch(true)"
        >
          重试
        </button>
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
        <div class="search-loading-text">
          正在聚合来源…
        </div>
        <div
          v-for="n in 3"
          :key="n"
          class="skeleton-row"
        >
          <span class="skeleton-bar skeleton-title" />
          <span class="skeleton-bar skeleton-meta" />
        </div>
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
              <span v-if="isDead(item)" class="dead-tag">分享失效</span>
              <span v-if="item.size">{{ formatBytes(item.size) }}</span>
              <span v-if="item.publishedAt">{{ formatTime(item.publishedAt) }}</span>
              <span class="result-sources">{{ item.sources.join(' · ') }}</span>
            </div>
            <div class="result-link">
              {{ item.shareUrl }}
              <button
                v-if="item.password"
                class="password-chip"
                type="button"
                title="复制提取码"
                @click="copyPassword(item)"
              >
                提取码 {{ item.password }}
              </button>
            </div>
          </div>
          <div class="result-actions">
            <el-button
              circle
              class="act-play"
              :disabled="!item.playable || isDead(item) || playingId === item.id"
              :title="!item.playable ? '暂不支持磁力/电驴播放' : isDead(item) ? '分享链接已失效' : '播放'"
              :aria-label="isDead(item) ? '已失效' : '播放'"
              @click="play(item)"
            >
              <template #icon><SolarIcon name="play" :size="16" /></template>
            </el-button>
            <el-button
              circle
              class="act-copy"
              title="复制链接"
              aria-label="复制链接"
              @click="copyLink(item)"
            >
              <template #icon><SolarIcon name="copy" :size="16" /></template>
            </el-button>
            <el-button
              circle
              class="act-info"
              title="详情"
              aria-label="详情"
              @click="detail = item"
            >
              <template #icon><SolarIcon name="info-circle" :size="16" /></template>
            </el-button>
          </div>
        </article>
      </div>

      <EmptyState
        v-else-if="result && !result.items.length"
        title="没有找到相关云盘资源"
        :desc="`没有找到与「${keyword}」相关的云盘资源，试试更短的关键词。`"
      />

      <div
        v-if="result && result.items.length"
        class="search-pager"
      >
        <label class="page-size">
          每页
          <select
            :model-value="pageSize"
            @change="onPageSizeChange"
          >
            <option :value="20">
              20
            </option>
            <option :value="30">
              30
            </option>
            <option :value="50">
              50
            </option>
          </select>
        </label>
        <template v-if="pageCount > 1">
          <button
            class="pager-btn"
            type="button"
            :disabled="page <= 1"
            @click="goPage(page - 1)"
          >
            上一页
          </button>
          <span class="pager-info">第 {{ page }} / {{ pageCount }} 页</span>
          <button
            class="pager-btn"
            type="button"
            :disabled="page >= pageCount"
            @click="goPage(page + 1)"
          >
            下一页
          </button>
        </template>
      </div>

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
          <span class="detail-label">提取码</span>
          <span>
            <button
              v-if="detail.password"
              class="password-chip"
              type="button"
              title="复制提取码"
              @click="copyPassword(detail)"
            >
              {{ detail.password }}
            </button>
            <template v-else>—</template>
          </span>
          <span class="detail-label">来源</span><span>{{ detail.sources.join(' · ') || '—' }}</span>
          <span class="detail-label">时间</span><span>{{ formatTime(detail.publishedAt) || '—' }}</span>
          <span class="detail-label">状态</span>
          <span :class="isDead(detail) ? 'detail-status is-dead' : 'detail-status'">{{ statusOf(detail) }}</span>
        </div>
        <template #footer>
          <el-button @click="detail && copyLink(detail)">
            复制链接
          </el-button>
          <el-button @click="detail && openExternal(detail)">
            打开原页
          </el-button>
          <span
            class="transfer-wrap"
            :title="detail && transferAllowed(detail.provider) ? '转存到我的网盘' : '转存功能按 provider 灰度，当前暂不支持'"
          >
            <el-button :disabled="!detail || !transferAllowed(detail.provider)">
              转存到我的网盘
            </el-button>
          </span>
          <el-button
            type="primary"
            :disabled="!detail?.playable || (!!detail && playingId === detail.id)"
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
.search-loading-text {
  font-size: 13px;
  color: var(--text-muted);
}

.skeleton-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 14px 16px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md, 8px);
  background: var(--surface);
}

.skeleton-bar {
  display: block;
  height: 12px;
  border-radius: 6px;
  background: linear-gradient(90deg, var(--border) 25%, var(--surface) 50%, var(--border) 75%);
  background-size: 200% 100%;
  animation: skeleton-pulse 1.2s ease-in-out infinite;
}

.skeleton-title {
  width: 56%;
  height: 14px;
}

.skeleton-meta {
  width: 32%;
}

@keyframes skeleton-pulse {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

.search-pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 18px 0 6px;
}

.pager-btn {
  padding: 6px 14px;
  border-radius: var(--radius-md, 8px);
  border: 1px solid var(--border);
  background: var(--surface);
  color: inherit;
  cursor: pointer;
}

.pager-btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.pager-info {
  font-size: 13px;
  opacity: 0.8;
}

.page-size {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  opacity: 0.8;
}

.page-size select {
  padding: 4px 8px;
  border-radius: var(--radius-md, 8px);
  border: 1px solid var(--border);
  background: var(--surface);
  color: inherit;
}

.detail-status {
  color: var(--accent);
}

.detail-status.is-dead {
  color: var(--warning);
}

.transfer-wrap {
  display: inline-flex;
  margin-left: 12px;
}

.dead-tag {
  padding: 1px 7px;
  border-radius: var(--radius-pill);
  border: 1px solid var(--warning);
  color: var(--warning);
  font-size: 11px;
}

/* 与其它内容页统一：内边距与块间距由全局 .page 提供 */
.cloud-search-page {
  min-height: 100%;
}

.search-top {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  flex-shrink: 0;
}

.search-left {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  min-width: 0;
}

.search-bar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  padding: var(--space-2);
}

.search-bar :deep(.search-input) {
  flex: 1;
  min-width: 0;
}

.back-btn {
  flex-shrink: 0;
}

.stats-sort-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
  flex-shrink: 0;
  padding: 2px 6px;
  border: none;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.stats-sort-btn:hover {
  color: var(--accent);
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

.filter-count {
  margin-left: 4px;
  font-size: 11px;
  opacity: 0.72;
}

.filter-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.search-error {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.search-error-detail {
  color: var(--text-muted);
  font-size: 12px;
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

.password-chip {
  padding: 1px 8px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.password-chip:hover {
  border-color: var(--accent);
  color: var(--text);
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
  align-self: center;
  gap: var(--space-2);
}

/* 播放/复制/详情 三色区分（禁用时保持灰） */
.result-actions .act-play:not(.is-disabled) {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-weak);
}
.result-actions .act-copy:not(.is-disabled) {
  color: #2f7fd6;
  border-color: #2f7fd6;
  background: rgba(47, 127, 214, 0.12);
}
.result-actions .act-info:not(.is-disabled) {
  color: #b7791f;
  border-color: #b7791f;
  background: rgba(183, 121, 31, 0.12);
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