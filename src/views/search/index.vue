<!-- 搜索页（全站点搜索）：左右布局
     上：搜索输入框 / 搜索历史（左）+ 热搜 tag（右）
     下：3/7 布局 —— 左侧站点列表，右侧当前站点结果卡片；全部滚动交给 el-scrollbar，
     结果没有分页按钮，滚到底部自动追加下一页；刷新按钮为圆形图标按钮。
     KeepAlive include="SearchView"：点进详情再返回时不重新挂载 / 不重新搜索，
     结果卡片、选中站点、已加载页数与滚动位置全部保持。 -->
<script setup lang="ts">
import { computed, nextTick, onActivated, onDeactivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import type { ScrollbarInstance } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import SearchInput from './components/SearchInput.vue'
import KeywordHistory from './components/KeywordHistory.vue'
import HotSearch from './components/HotSearch.vue'
import VideoGrid from '@/components/VideoGrid.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import { useConfigStore } from '@/store/config'
import { useSearchStore, type SearchSelection, type SearchSiteRef } from '@/store/search'
import { useUiStore } from '@/store/ui'
import { classifySite } from '@/utils/siteSupport'
import type { SearchHit, VideoItem } from '@/types'

/** KeepAlive 白名单按组件名匹配（见 layouts/default）：必须与 include 里的名字一致 */
defineOptions({ name: 'SearchView' })

const router = useRouter()
const route = useRoute()
const config = useConfigStore()
const searchStore = useSearchStore()
const ui = useUiStore()

const inputRef = ref<InstanceType<typeof SearchInput> | null>(null)
const resultScrollRef = ref<ScrollbarInstance | null>(null)
const siteScrollRef = ref<ScrollbarInstance | null>(null)

/* ---------------- KeepAlive 缓存：结果与滚动位置 ----------------
   本页在 default 布局里被 KeepAlive 缓存（include="SearchView"）：
   点进详情再返回时不重新挂载、不重新请求，结果 / 选中站点 / 已加载页数都原样保留。
   页面内部是独立 el-scrollbar（内容区外层容器不滚动），DOM 停用后重新插入会丢掉
   scrollTop，所以进入时按离开前的位置恢复。 */
let pageActive = false
/** 已经处理过的 route.query.q：从详情返回时 q 不变，不能因此重复搜索 */
let appliedQuery: string | null = null
/** 结果区 / 站点列表离开前的滚动位置（滚动事件里实时记录） */
let savedResultScroll = 0
let savedSiteScroll = 0
/** 正在按 q 搜索的关键词：onMounted / onActivated 连续触发时避免重复发请求 */
let pendingQuery = ''

/** 可搜索站点：与后端 search_all 的过滤保持一致（searchable=true 且非内置推送 type=-1），
 *  再排除本机不可加载的源（jar 等）——这些站点无法在本机发起搜索。 */
const searchSites = computed<SearchSiteRef[]>(() =>
  config.sortedSites
    .filter((site) => site.type !== -1 && site.searchable !== false && classifySite(site).local)
    .map((site) => ({ key: site.key, name: site.name })),
)

/** 当前选中项标题（结果头左侧「搜索的名称」） */
const selectedLabel = computed(() => {
  if (searchStore.selectedKey === 'all') return '全部站点'
  return searchStore.selectedGroup?.name ?? '站点'
})

/** 当前是否正在刷新（全量搜索或单站刷新） */
const refreshing = computed(
  () => searchStore.loading || Boolean(searchStore.selectedGroup?.loading),
)

/** 站点 key → 名称：结果卡片上标注该条来自哪个站点（全部站点聚合时尤其需要） */
const siteNameByKey = computed(() => {
  const map = new Map<string, string>()
  for (const group of searchStore.groups) map.set(group.key, group.name)
  return map
})

/** 当前选中项已展示的结果（给 id 加站点/下标前缀，避免跨站同名资源撞 key） */
const resultItems = computed<VideoItem[]>(() => {
  const entries: { groupKey: string; hit: SearchHit }[] = []
  if (searchStore.selectedKey === 'all') {
    for (const group of searchStore.orderedGroups) {
      for (const hit of group.hits) entries.push({ groupKey: group.key, hit })
    }
  } else {
    const group = searchStore.selectedGroup
    if (group) for (const hit of group.hits) entries.push({ groupKey: group.key, hit })
  }
  return entries
    .slice(0, searchStore.visibleCount)
    .map(({ groupKey, hit }, index) => ({
      ...hit,
      id: `${groupKey}:${index}:${hit.id}`,
      siteName: siteNameByKey.value.get(groupKey) ?? groupKey,
    }))
})

/** 当前可见结果对应的原始命中（点击卡片时按 id 反查站点） */
const visibleHits = computed<{ groupKey: string; hit: SearchHit }[]>(() => {
  const entries: { groupKey: string; hit: SearchHit }[] = []
  if (searchStore.selectedKey === 'all') {
    for (const group of searchStore.orderedGroups) {
      for (const hit of group.hits) entries.push({ groupKey: group.key, hit })
    }
  } else {
    const group = searchStore.selectedGroup
    if (group) for (const hit of group.hits) entries.push({ groupKey: group.key, hit })
  }
  return entries.slice(0, searchStore.visibleCount)
})

/** 空结果文案：区分「全部站点都没命中」与「当前站点没命中」 */
const emptyDesc = computed(() => {
  if (searchStore.selectedKey !== 'all') {
    return '该站点没有匹配结果，可以切到「全部」看看，或在设置页增加搜索线程后重试'
  }
  return '所有站点都没有匹配结果：试试更短的关键词，或在设置页调整搜索线程后刷新'
})

/** 当前选中项是否仍在等待结果（用于决定显示骨架还是空态） */
const selectedPending = computed(() =>
  searchStore.selectedKey === 'all'
    ? searchStore.loading
    : Boolean(searchStore.selectedGroup?.loading),
)

/** 结果底部提示：边搜边展示时明确告诉用户“还在继续搜” */
const streamingHint = computed(() => {
  if (selectedPending.value) {
    return searchStore.selectedKey === 'all' ? '正在搜索其它站点…' : '正在搜索该站点…'
  }
  if (searchStore.hasMore) return '正在加载下一页…'
  return '已经到底啦'
})

onMounted(() => {
  pageActive = true
  // 热搜异步加载，不阻塞搜索
  void searchStore.loadHotKeywords()
  // 首次进入：/search?q=片名（首页豆瓣卡片）自动搜索；无 q 则保持初始态
  applyQuerySearch()
})

onActivated(() => {
  pageActive = true
  // KeepAlive 复用：只有 q 变化才重新搜索；从详情返回时 q 不变，直接复用缓存结果
  applyQuerySearch()
  restoreScroll()
})

onDeactivated(() => {
  pageActive = false
})

/**
 * 按路由 query.q 触发搜索（等配置初始化完成再搜）。
 * 同一个 q 只处理一次：从详情返回、再次点同一张豆瓣卡片都复用已有结果，不再发请求。
 */
function applyQuerySearch() {
  const keyword = readQueryKeyword()
  if (keyword === appliedQuery) {
    // q 没变（多为从详情返回）：结果还在就直接复用；结果被清掉（如中途切换线路）才按 q 补搜
    if (keyword && !searchStore.searched && !searchStore.loading && pendingQuery !== keyword) {
      searchStore.setKeyword(keyword)
      void searchFromQuery(keyword)
    }
    return
  }
  appliedQuery = keyword
  // 无 q：保留当前结果（从详情返回 / 侧栏进入都走这里）
  if (!keyword) return
  // 该关键词已经搜过：只回填输入框，展示缓存结果
  if (searchStore.searched && searchStore.lastKeyword === keyword) {
    searchStore.setKeyword(keyword)
    return
  }
  searchStore.setKeyword(keyword)
  void searchFromQuery(keyword)
}

/** 读取 route.query.q（可能是数组 / 空值） */
function readQueryKeyword(): string {
  const raw = route.query.q
  const value = typeof raw === 'string' ? raw : Array.isArray(raw) ? (raw[0] ?? '') : ''
  return value.trim()
}

/** 带 q 进入搜索页：等配置初始化完成再搜索 */
async function searchFromQuery(keyword: string) {
  if (pendingQuery === keyword) return
  pendingQuery = keyword
  try {
    for (let i = 0; i < 30 && !config.initialized; i += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 100))
    }
    await runSearch()
  } finally {
    if (pendingQuery === keyword) pendingQuery = ''
  }
}

/* ---------------- 滚动位置：KeepAlive 停用后恢复 ---------------- */

function scrollWrap(scroller: ScrollbarInstance | null): HTMLElement | undefined {
  const el = scroller?.$el as HTMLElement | undefined
  return el?.querySelector<HTMLElement>('.el-scrollbar__wrap') ?? undefined
}

/** 只在当前还在更上方时回填，避免和用户返回后的手动滚动打架 */
function restoreOne(scroller: ScrollbarInstance | null, target: number) {
  if (!scroller || target <= 0) return
  if ((scrollWrap(scroller)?.scrollTop ?? 0) >= target) return
  scroller.setScrollTop(target)
}

/** 缓存页面重新插入后恢复结果区 / 站点列表的滚动位置 */
function restoreScroll() {
  void nextTick(() => {
    restoreOne(resultScrollRef.value, savedResultScroll)
    restoreOne(siteScrollRef.value, savedSiteScroll)
    // 卡片高度可能受图片 / 字体二次布局影响，稍后再补一次（不会盖住用户的新位置）
    window.setTimeout(() => {
      restoreOne(resultScrollRef.value, savedResultScroll)
      restoreOne(siteScrollRef.value, savedSiteScroll)
    }, 80)
  })
}

/** KeepAlive 复用期间 q 变化（再次从首页豆瓣卡片进入）时按新关键词搜索 */
watch(
  () => route.query.q,
  () => {
    if (!pageActive || route.name !== 'search') return
    applyQuerySearch()
  },
)

/** 离开搜索页去别的页面（详情页除外）时清掉 q 记忆：下次带 q 进入按新入口重新搜索 */
watch(
  () => route.name,
  (name, prev) => {
    if (prev !== 'search' || name === 'search' || name === 'detail') return
    appliedQuery = null
  },
)

/* ---------------- 搜索动作 ---------------- */

async function runSearch() {
  const keyword = searchStore.keyword.trim()
  if (!keyword) {
    ui.notify('请输入关键词后再搜索', 'warning')
    inputRef.value?.focus()
    return
  }
  // 首次进入时配置可能还没加载完：等一小会再判断“没有可搜索站点”
  for (let i = 0; i < 30 && !config.initialized && !searchSites.value.length; i += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 100))
  }
  if (!searchSites.value.length) {
    ui.notify('当前线路没有可搜索的本机站点', 'warning')
    return
  }
  await searchStore.searchAll(searchSites.value)
  scrollResultTop()
}

async function onRefresh() {
  if (refreshing.value) return
  await searchStore.refreshCurrent(searchSites.value)
  scrollResultTop()
}

function onKeywordUpdate(value: string) {
  searchStore.setKeyword(value)
}

/** 点击历史 / 热搜：回填并立即搜索 */
function onPick(keyword: string) {
  searchStore.setKeyword(keyword)
  void runSearch()
}

function onRemove(keyword: string) {
  searchStore.removeKeyword(keyword)
}

function onClearHistory() {
  searchStore.clearHistory()
  ui.notify('已清空搜索历史', 'success')
}

/** Esc / 清空：输入与结果一起复位，回到初始态 */
function onClearInput() {
  searchStore.reset()
  inputRef.value?.focus()
}

function focusInput() {
  inputRef.value?.focus()
}

/** 点击站点：切换右侧结果并回到顶部 */
function onSelectSite(key: SearchSelection) {
  if (searchStore.selectedKey === key) return
  searchStore.selectSite(key)
  scrollResultTop()
}

function scrollResultTop() {
  // 新结果从顶部开始：同步清掉记住的位置，避免返回时又滚回旧位置
  savedResultScroll = 0
  void nextTick(() => resultScrollRef.value?.setScrollTop(0))
}

/* ---------------- 结果滚动：到底部自动加载下一页 ---------------- */

function resultWrap(): HTMLElement | undefined {
  return scrollWrap(resultScrollRef.value)
}

function onResultScroll(payload: { scrollTop: number; scrollLeft: number }) {
  if (!pageActive) return
  // 记录位置：KeepAlive 停用后 DOM 重新插入会丢 scrollTop
  savedResultScroll = payload.scrollTop
  const wrap = resultWrap()
  if (!wrap) return
  // 距离底部 120px 内视为到底：追加一页（本地分页，无需再请求）
  if (wrap.scrollHeight - payload.scrollTop - wrap.clientHeight < 120) {
    searchStore.loadMore()
  }
}

function onSiteScroll(payload: { scrollTop: number; scrollLeft: number }) {
  if (!pageActive) return
  savedSiteScroll = payload.scrollTop
}

/** 选中站点变化后回到结果顶部 */
watch(
  () => searchStore.selectedKey,
  () => scrollResultTop(),
)

/** 切换线路后旧结果属于上一条线路，直接回到初始态 */
watch(
  () => config.enabled,
  (next, prev) => {
    if (prev && next && next !== prev) searchStore.resetResults()
  },
)

/** 结果总量变化（重新搜索）时回到顶部 */
watch(
  () => searchStore.searched,
  () => scrollResultTop(),
)

/** 点击结果卡片：按命中项自带的 siteKey 进入详情 */
function onSelect(item: VideoItem) {
  const entry = visibleHits.value.find(({ groupKey, hit }, index) => {
    return `${groupKey}:${index}:${hit.id}` === item.id
  })
  if (!entry) return
  void router.push({
    name: 'vod-detail',
    params: { site: entry.groupKey, id: entry.hit.id },
  })
}

/** 返回点播父级首页：搜索是 /vod 的子路由，不跟随浏览器历史回到其他一级菜单 */
function goBack() {
  void router.push('/vod')
}
</script>

<template>
  <div class="page search-page">
    <!-- 顶部：左（搜索输入框 + 搜索历史） / 右（热搜条目，跨两行） -->
    <section class="search-top">
      <div class="search-left">
        <section class="search-bar panel">
          <el-button
            circle
            aria-label="返回点播首页"
            @click="goBack"
          >
            <template #icon><SolarIcon name="arrow-left" /></template>
          </el-button>
          <SearchInput
            ref="inputRef"
            :model-value="searchStore.keyword"
            :loading="searchStore.loading"
            @update:model-value="onKeywordUpdate"
            @search="runSearch"
            @clear="onClearInput"
          />
        </section>
        <KeywordHistory
          :items="searchStore.history"
          @pick="onPick"
          @remove="onRemove"
          @clear="onClearHistory"
        />
      </div>
      <HotSearch
        class="hot-panel"
        :items="searchStore.hotKeywords"
        :loading="searchStore.hotLoading"
        @pick="onPick"
      />
    </section>

    <!-- 结果：3/7 布局（左站点 / 右当前站点卡片） -->
    <section class="search-results">
      <!-- 左：站点列表 -->
      <aside class="site-nav panel" aria-label="搜索站点">
        <el-scrollbar
          ref="siteScrollRef"
          class="site-scroll"
          @scroll="onSiteScroll"
        >
          <button
            type="button"
            class="site-item"
            :class="{ active: searchStore.selectedKey === 'all' }"
            @click="onSelectSite('all')"
          >
            <span class="site-name">全部</span>
            <span class="site-count">{{ searchStore.totalCount }}</span>
          </button>
          <button
            v-for="group in searchStore.groups"
            :key="group.key"
            type="button"
            class="site-item"
            :class="{ active: searchStore.selectedKey === group.key }"
            :title="group.name"
            @click="onSelectSite(group.key)"
          >
            <span class="site-name">{{ group.name }}</span>
            <span v-if="group.loading" class="site-loading" aria-label="搜索中" />
            <span v-else class="site-count" :class="{ zero: !group.total && !group.hits.length }">
              {{ group.total || group.hits.length }}
            </span>
          </button>
        </el-scrollbar>
      </aside>

      <!-- 右：当前站点结果 -->
      <section class="result-pane panel" aria-label="搜索结果">
        <header class="result-head">
          <div class="result-title">
            <span class="result-keyword">
              搜索「{{ searchStore.lastKeyword || searchStore.keyword || '—' }}」
            </span>
            <span class="result-meta">
              {{ searchStore.selectedTotal }} 条 · {{ selectedLabel }}
              <template v-if="searchStore.selectedKey === 'all'">
                · {{ searchStore.hitSiteCount }} 个站点命中
              </template>
            </span>
            <el-tag v-if="config.demoMode" effect="light" type="warning">演示数据</el-tag>
          </div>
          <el-tooltip content="刷新当前结果" placement="left">
            <el-button
              circle
              class="refresh-btn"
              :disabled="!searchStore.searched || !searchStore.lastKeyword || refreshing"
              :aria-label="`刷新${selectedLabel}搜索结果`"
              @click="onRefresh"
            >
              <template #icon>
                <SolarIcon name="refresh" :class="{ spinning: refreshing }" />
              </template>
            </el-button>
          </el-tooltip>
        </header>

        <el-scrollbar
          ref="resultScrollRef"
          class="result-scroll"
          @scroll="onResultScroll"
        >
          <div class="result-body">
            <!-- ① 全局搜索失败（如当前线路没有可搜索站点） -->
            <ErrorState
              v-if="searchStore.error"
              :message="searchStore.error"
              title="搜索失败"
              retry-text="重试"
              @retry="runSearch"
            />

            <!-- ② 已搜索：结果边到边展示，不再等所有站点一次性返回 -->
            <template v-else-if="searchStore.searched">
              <!-- 单站刷新失败 -->
              <ErrorState
                v-if="searchStore.selectedGroup?.error && !resultItems.length"
                :message="searchStore.selectedGroup.error"
                title="站点搜索失败"
                retry-text="重试"
                @retry="onRefresh"
              />

              <!-- 还没有任何结果、仍在检索：骨架 -->
              <SkeletonGrid v-else-if="!resultItems.length && selectedPending" :count="12" />

              <!-- 搜索结束且没有结果 -->
              <EmptyState
                v-else-if="!resultItems.length"
                title="没有找到相关结果"
                :desc="emptyDesc"
                action-text="换个关键词"
                @action="focusInput"
              />

              <!-- 结果卡片 + 滚动到底自动加载下一页 -->
              <template v-else>
                <VideoGrid :items="resultItems" @select="onSelect" />
                <p class="load-more">{{ streamingHint }}</p>
              </template>
            </template>

            <!-- ③ 初始态 -->
            <EmptyState
              v-else
              title="输入关键词开始搜索"
              desc="默认全站点搜索：输入片名 / 演员 / 关键词，按 Enter 直接搜索；结果按站点分组展示"
            />
          </div>
        </el-scrollbar>
      </section>
    </section>
  </div>
</template>

<style scoped>
/* 页面固定一屏：顶部搜索区 + 下方结果区各自内部滚动（内容区外层 el-scrollbar 不再滚动） */
.search-page {
  height: calc(100vh - var(--titlebar-h));
  box-sizing: border-box;
  overflow: hidden;
}

/* —— 顶部左右布局（固定高度：右侧热搜内部滚动，左侧历史铺满剩余空间） —— */
.search-top {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(260px, 30%);
  gap: var(--space-4);
  flex-shrink: 0;
  height: 208px;
  min-height: 0;
}

.search-left {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  min-width: 0;
}

.search-bar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-shrink: 0;
  min-width: 0;
}

.search-bar :deep(.search-input) {
  flex: 1;
  min-width: 0;
}


.hot-panel {
  min-width: 0;
  min-height: 0;
}

/* —— 结果 3/7 布局 —— */
.search-results {
  display: grid;
  grid-template-columns: minmax(180px, 3fr) 7fr;
  gap: var(--space-4);
  flex: 1;
  min-height: 0;
}

.panel {
  min-height: 0;
}

/* 左：站点列表（内边距跟 .panel 一致，12px） */
.site-nav {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.site-scroll {
  flex: 1;
  min-height: 0;
}

.site-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  width: 100%;
  min-width: 0;
  margin: 0;
  padding: 7px var(--space-2);
  border: 0;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-regular);
  font: inherit;
  font-size: 13px;
  text-align: left;
  cursor: pointer;
}

.site-item:hover {
  background: var(--surface-3);
  color: var(--text);
}

.site-item.active {
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

.site-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.site-count {
  flex-shrink: 0;
  min-width: 22px;
  padding: 0 5px;
  border-radius: var(--radius-pill);
  background: var(--surface-3);
  color: var(--text-muted);
  font-size: 11px;
  line-height: 18px;
  text-align: center;
  font-variant-numeric: tabular-nums;
}

.site-item.active .site-count {
  background: var(--accent-weak-hover);
  color: var(--accent);
}

.site-count.zero {
  opacity: 0.55;
}

/* 单站检索中：静态浅色轨道 + 旋转的高亮弧（手绘圆环观感）。
   高亮弧不再用 border-top-color 旋转——小尺寸边框每帧重新栅格化，转起来会「一甩一甩」、
   不是正圆；这里用锥形渐变画 90° 弧、径向 mask 裁成 2px 圆环，旋转时是整块图层，边缘稳定。 */
.site-loading {
  position: relative;
  flex-shrink: 0;
  box-sizing: border-box;
  width: 18px;
  height: 18px;
  border: 2px solid var(--accent-weak-hover);
  border-radius: 50%;
}

.site-loading::after {
  content: '';
  position: absolute;
  inset: -2px;
  border-radius: 50%;
  background: conic-gradient(
    from 315deg,
    var(--accent) 0deg 90deg,
    transparent 90deg 360deg
  );
  -webkit-mask: radial-gradient(
    farthest-side,
    transparent calc(100% - 2px),
    #000 calc(100% - 2px)
  );
  mask: radial-gradient(
    farthest-side,
    transparent calc(100% - 2px),
    #000 calc(100% - 2px)
  );
  animation: spin 0.8s linear infinite;
}

/* 右：结果面板 */
.result-pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
}

.result-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  padding: 0 0 var(--space-3);
  border-bottom: 1px solid var(--border-lighter);
  flex-shrink: 0;
}

.result-title {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
  min-width: 0;
}

.result-keyword {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 100%;
}

.result-meta {
  font-size: 12px;
  color: var(--text-muted);
}

.refresh-btn {
  flex-shrink: 0;
}

.result-scroll {
  flex: 1;
  min-height: 0;
}

.result-body {
  padding: var(--space-3) 0;
  padding-bottom: 0;
}

.load-more {
  margin: var(--space-3) 0 0;
  color: var(--text-faint);
  font-size: 12px;
  text-align: center;
}

.spinning {
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .spinning,
  .site-loading::after {
    animation: none;
  }
}
</style>
