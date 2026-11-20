<!-- 点播页（frontend-design.md §6.1，布局按用户要求调整）：
     顶部单行栏 —— [首页 + 源分类标签（横向滚动，flex:1）] | [源切换] | [搜索] | [刷新]
     首页：豆瓣热播轮播（10 条）+ 豆瓣分类标签 + 卡片网格；选中源分类则是该源的视频列表。
     KeepAlive include="VodView" 保存分类/分页/滚动位置 -->
<script setup lang="ts">
import { computed, onActivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import * as api from '@/api'
import StatusTabs from '@/components/StatusTabs.vue'
import DoubanHome from './components/DoubanHome.vue'
import ContinueWatching from './components/ContinueWatching.vue'
import VideoGrid from '@/components/VideoGrid.vue'
import Pager from './components/Pager.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import { useConfigStore } from '@/store/config'
import { useHistoryStore } from '@/store/history'
import { useUiStore } from '@/store/ui'
import type { Category, DoubanHome as DoubanHomeData, DoubanItem, HistoryItem, SourceTabItem, VideoItem } from '@/types'

defineOptions({ name: 'VodView' })

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const history = useHistoryStore()
const ui = useUiStore()

/* ---------------- 页面状态 ---------------- */

const categories = ref<Category[]>([])
const items = ref<VideoItem[]>([])
/** 当前分类 type_id（'' = 首页） */
const category = ref('')
const page = ref(1)
const hasNext = ref(false)
const catLoading = ref(false)
const listLoading = ref(false)
const error = ref('')
/** 分类接口失败原因：与「真空分类」区分，避免把分类接口故障显示成「该分类暂无内容」 */
const catError = ref('')

/** 首页（豆瓣）数据状态 */
const home = ref<DoubanHomeData | null>(null)
const homeLoading = ref(false)
const homeError = ref('')
/** 首页数据是否已成功加载过（避免站点切换时重复整页请求） */
let homeLoaded = false
/** 首页请求进行中（模块级并发保护；缓存渲染阶段不置 homeLoading，所以不能只靠它） */
let homeInflight = false

let reqSeq = 0
/** 当前 categories 属于哪个站点（识别站点切换：先清旧分类再读缓存） */
let categoriesSite = ''
/** 已加载过的站点，避免同一站点重复请求 */
let loadedSite = ''
/** 已加载过的启用地址（切换订阅源后即使站点 key 相同也要重载） */
let loadedEnabled = ''

/* ---------------- 派生状态 ---------------- */

/** 当前是否在「首页」（分类为空串） */
const isHome = computed(() => category.value === '')

/** 当前点播站点 key（来自 config 的站点记忆） */
const siteKey = computed(() => config.activeSiteKey('vod'))
const site = computed(() => (siteKey.value ? config.siteByKey(siteKey.value) : undefined))
const siteType = computed(() => site.value?.type ?? -1)
/** 当前站点的能力信息：type3 jar/Python 在桌面端需后续支持，由 store 统一判定 */
const siteSupport = computed(() =>
  config.sourceTabs.find((tab) => tab.key === siteKey.value),
)
const siteUnsupported = computed(() => Boolean(site.value) && siteSupport.value?.supported === false)
/** 本机不可用时的错误态标题 / 说明（区分 jar、Python、其它类型） */
const unsupportedTitle = computed(() =>
  siteSupport.value?.level === 'jar' || siteSupport.value?.level === 'python'
    ? '该源需要后续支持'
    : '该源类型暂未适配',
)
const unsupportedMessage = computed(() => {
  const tab = siteSupport.value
  if (!tab) return ''
  if (tab.level === 'jar') {
    return '该线路为 Android jar 源，桌面端后续支持（需 Android Host），请切换线路'
  }
  if (tab.level === 'python') {
    return '该站点使用 Python spider，后续支持（需系统 Python 运行时），可先切换其它站点'
  }
  return `${tab.reason ?? `暂未适配（type=${siteType.value}）`}；本机已支持 XML / 苹果CMS / HTTP 远程接口 / JS spider / 原生规则引擎（XYQHiker、XBPQ）/ Python（需系统 Python）`
})

/** 初始化中（配置尚未读取完成） */
const initializing = computed(() => !config.initialized || (config.loading && !config.sites.length))
/** 无订阅 */
const noSubscription = computed(() => config.initialized && config.subscriptions.length === 0)
/** 有订阅但未启用地址 */
const noEnabled = computed(
  () => config.initialized && config.subscriptions.length > 0 && !config.enabled,
)
/** 已启用但没有任何站点 */
const noSites = computed(
  () => config.initialized && Boolean(config.enabled) && config.sites.length === 0,
)

/* ---------------- 空态文案（区分「分类接口失败」「全部为空」「该分类为空」） ---------------- */

/** 分类接口失败且没有任何分类可切换 */
const categoriesUnavailable = computed(() => Boolean(catError.value) && !categories.value.length)
/** 空列表标题 */
const emptyTitle = computed(() => {
  if (categoriesUnavailable.value) return '分类加载失败'
  return '该分类暂无内容'
})
/** 空列表说明 */
const emptyDesc = computed(() => {
  if (categoriesUnavailable.value) {
    return `无法获取分类：${catError.value}；可重试，或回到首页浏览豆瓣热播`
  }
  return '可以切换其它分类，或回到首页浏览豆瓣热播'
})
/** 空列表操作文案：分类接口失败时给「重试」，否则回首页 */
const emptyActionText = computed(() => (categoriesUnavailable.value ? '重试' : '回到首页'))

/** 当前站点名（顶部站点按钮展示） */
const currentSiteName = computed(() => site.value?.name ?? '')
/** 站点选择弹窗 */
const siteDialogVisible = ref(false)
const siteKeyword = ref('')
/** 弹窗内站点列表：可用源在前（config.sourceTabs 已排序），仅按关键字过滤名称 / key */
const filteredSites = computed<SourceTabItem[]>(() => {
  const kw = siteKeyword.value.trim().toLowerCase()
  if (!kw) return config.sourceTabs
  return config.sourceTabs.filter(
    (s) => s.name.toLowerCase().includes(kw) || s.key.toLowerCase().includes(kw),
  )
})

/* ---------------- 继续观看（来自观看历史，最多 6 条） ---------------- */

/** 最近观看记录：store 已按 updatedAt 倒序并截取 6 条 */
const continueItems = computed<HistoryItem[]>(() => history.continueItems)

/** 点击继续观看：进详情页并带上「第几集 + 已看秒数」，由详情页自动定位并续播 */
function onContinuePlay(item: HistoryItem) {
  void router.push({
    name: 'vod-detail',
    params: { site: item.siteKey, id: item.id },
    query: {
      ep: String(item.epIndex),
      t: String(Math.max(0, Math.round(item.positionSec || 0))),
    },
  })
}

/* ---------------- 数据加载 ---------------- */

/** 拉取当前站点的分类（失败不阻断列表，仅提示） */
async function loadCategories(force = false) {
  const key = siteKey.value
  if (!key) {
    categories.value = []
    categoriesSite = ''
    return
  }
  catError.value = ''
  // 站点切换：先清掉旧站点分类（避免残留），再读磁盘缓存立即展示（不设 catLoading，避免 tab 骨架闪）
  if (categoriesSite !== key) {
    categoriesSite = key
    categories.value = []
    try {
      const cached = await api.listCategoriesCached(key)
      if (key !== siteKey.value) return
      if (cached?.length) categories.value = cached
    } catch {
      // 缓存读取失败忽略：继续走网络
    }
  }
  catLoading.value = true
  try {
    const res = await api.listCategories(key, force)
    if (key !== siteKey.value) return
    categories.value = res
    // 当前分类在新站点不存在时回退到「首页」
    if (category.value && !res.some((c) => c.type_id === category.value)) category.value = ''
  } catch (e) {
    categories.value = []
    catError.value = api.formatError(e)
    ui.notify('分类加载失败：' + catError.value, 'error')
    // 源健康度：连续失败达到阈值 → 自动切到下一个未禁用的可用源
    const currentKey = config.activeSite?.key ?? ''
    if (currentKey) {
      await config.loadHealth()
      if (config.isSiteDisabled(currentKey)) {
        const next = config.nextHealthySite(currentKey)
        if (next) {
          config.setActiveSite('vod', next)
          ui.notify(
            `当前源连续失败，已自动切换源：${config.siteByKey(next)?.name ?? next}`,
            'warning',
          )
        }
      }
    }
  } finally {
    catLoading.value = false
  }
}

/**
 * 拉取豆瓣首页数据（force=true 跳过缓存）。
 * 缓存优先：启动无数据时先读磁盘缓存立即渲染，再后台增量刷新；
 * 已有 home（含刷新按钮）时也先保留现有内容再刷新，避免空白/骨架闪一下。
 */
async function loadHome(force = false) {
  // 并发保护用模块级标记：读缓存阶段不置 homeLoading，不能只靠它
  if (homeInflight) return
  homeInflight = true
  homeError.value = ''
  try {
    if (!home.value) {
      // 首次/无数据：先读只读磁盘缓存（毫秒级、不联网），不设 homeLoading 以免骨架闪一下
      try {
        const cached = await api.doubanHomeCached()
        if (cached) {
          home.value = cached
          homeLoaded = true
        }
      } catch {
        // 缓存读取失败忽略：继续走网络
      }
    }
    homeLoading.value = true
    home.value = await api.doubanHome(force)
    homeLoaded = true
  } catch (e) {
    homeError.value = api.formatError(e)
    ui.notify(
      (home.value ? '首页刷新失败：' : '首页加载失败：') + homeError.value,
      'error',
    )
  } finally {
    homeLoading.value = false
    homeInflight = false
  }
}

/** 拉取当前分类 / 页码的列表；空数组即视为末页（前端判定，不改后端） */
async function loadList() {
  const key = siteKey.value
  if (!key) return
  const seq = ++reqSeq
  listLoading.value = true
  error.value = ''
  try {
    const res = await api.listVideos(key, category.value, page.value)
    if (seq !== reqSeq) return
    if (res.length === 0) {
      hasNext.value = false
      if (page.value > 1) {
        // 已到最后一页：回退页码并保留上一页内容
        page.value -= 1
        ui.notify('已到最后一页', 'info')
      } else {
        items.value = []
      }
    } else {
      items.value = res
      hasNext.value = true
    }
  } catch (e) {
    if (seq !== reqSeq) return
    error.value = api.formatError(e)
    items.value = []
    hasNext.value = false
  } finally {
    if (seq === reqSeq) listLoading.value = false
  }
}

/** 站点切换 / 手动刷新：分类、列表、页码全部重置（回到首页） */
async function loadAll(force = false) {
  page.value = 1
  category.value = ''
  items.value = []
  hasNext.value = false
  error.value = ''
  // 分类标签始终展示（即使当前在首页），无站点时只剩「首页」
  await loadCategories(force)
  if (isHome.value) {
    await loadHome(false)
  } else if (siteKey.value && !siteUnsupported.value) {
    await loadList()
  }
}

/** 首次进入或站点 / 启用源变化时确保加载 */
function ensureLoaded() {
  // 首页数据与站点无关：即使没有订阅也先拉豆瓣数据
  if (!homeLoaded && !homeLoading.value) void loadHome(false)
  const key = siteKey.value
  if (!key || siteUnsupported.value) return
  if (key === loadedSite && config.enabled === loadedEnabled) return
  loadedSite = key
  loadedEnabled = config.enabled
  void loadAll()
}

/** 列表加载失败后的重试（保留当前分类与页码） */
function retry() {
  if (catLoading.value || listLoading.value) return
  if (!categories.value.length) void loadCategories(true)
  void loadList()
}

/* ---------------- 交互 ---------------- */

/** 顶部分类 tab：内置「首页」+ 后端分类（与下载/设置页同款 StatusTabs） */
const categoryTabs = computed(() => [
  { value: '', label: '首页' },
  ...categories.value.map((c) => ({ value: c.type_id, label: c.type_name })),
])

function onCategorySelect(typeId: string) {
  if (typeId === category.value) return
  category.value = typeId
  page.value = 1
  hasNext.value = false
  items.value = []
  error.value = ''
  if (!typeId) {
    // 回到首页：已有数据直接展示，否则加载
    if (!home.value && !homeLoading.value) void loadHome(false)
    return
  }
  void loadList()
}

/**
 * 分类条滚轮：纵向滚轮 → 横向滚动。
 * el-scrollbar 只负责滚动条外观/拖拽，不会把纵向滚轮转成横向，这里补一行转换；
 * 分类未溢出时不拦截，滚轮照常滚页面。
 */
function onCatStripWheel(event: WheelEvent) {
  const wrap = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(
    '.el-scrollbar__wrap',
  )
  if (!wrap || wrap.scrollWidth <= wrap.clientWidth + 1) return
  event.preventDefault()
  wrap.scrollLeft += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY
}

function onPrev() {
  if (page.value <= 1 || listLoading.value) return
  page.value -= 1
  void loadList()
}

function onNext() {
  if (!hasNext.value || listLoading.value) return
  page.value += 1
  void loadList()
}

/** 空态「回到首页」 */
function goHome() {
  category.value = ''
  page.value = 1
  hasNext.value = false
  items.value = []
  error.value = ''
  if (!home.value && !homeLoading.value) void loadHome(false)
}

/** 空态按钮：分类接口失败时整体重试（分类 + 列表），否则回首页 */
function onEmptyAction() {
  if (categoriesUnavailable.value) {
    void loadAll(true)
    return
  }
  goHome()
}

function onSelect(item: VideoItem) {
  if (!siteKey.value) return
  // 分类名通过 history state 传给详情页（不改路由 URL；详情接口本身无分类字段）
  const categoryName = categories.value.find((c) => c.type_id === category.value)?.type_name ?? ''
  void router.push({
    name: 'vod-detail',
    params: { site: siteKey.value, id: item.id },
    state: { vodCategory: categoryName },
  })
}

/** 点击豆瓣条目：在当前点播源里按片名搜索（豆瓣只提供信息，不提供播放） */
function onDoubanSelect(item: DoubanItem) {
  const q = item.title.trim()
  if (!q) return
  void router.push({ path: '/vod/search', query: { q } })
}

function reload() {
  if (listLoading.value || catLoading.value || homeLoading.value) return
  if (isHome.value) {
    // 首页刷新：同时刷新豆瓣数据（强制跳过缓存）与分类标签
    void loadCategories(true)
    void loadHome(true)
    return
  }
  if (siteUnsupported.value) return
  // 分类视图刷新：保留当前分类与页码，只重拉分类标签和列表
  void (async () => {
    await loadCategories(true)
    // 分类在新数据里不存在时 loadCategories 会回退到首页，此时不再拉列表
    if (!isHome.value) await loadList()
  })()
}

/** 打开站点选择弹窗（多仓时弹窗内也会展示仓库切换） */
function openSiteDialog() {
  siteKeyword.value = ''
  // 进入弹窗前刷新源健康：设置页重置后首页/弹窗立即联动
  void config.loadHealth()
  siteDialogVisible.value = true
}

/** 在站点弹窗中选择站点：切换后关闭弹窗（仓库切换保持打开） */
/** 站点是否因连续失败被禁用（>=3 次，来自 source_health） */
function siteDisabled(key: string): boolean {
  return config.isSiteDisabled(key)
}

/** 点击被源健康禁用的站点：提示失败次数与重置入口 */
function onSiteHealthBlocked(site: SourceTabItem) {
  const record = config.healthOf('site', site.key)
  const reason = record?.lastError ? `：${record.lastError}` : ''
  ui.notify(
    `源「${site.name}」连续失败 ${record?.failures ?? 3} 次${reason}；可在 设置 → 源健康 重置`,
    'warning',
  )
}

/** 站点徽标文案：按 REA 画像给出「待移植 / 需 Python / 不支持 / 待评估」。 */
function siteBadge(site: SourceTabItem): string {
  if (site.supported) return ''
  if (site.level === 'jar') return '待移植'
  if (site.level === 'python') return '需 Python'
  if (site.level === 'unsupported') return '不支持'
  return '待评估'
}

/** 站点按钮 tooltip：本机可用给能力范围，不可用给 REA 画像原因（避免省略号看不到全文）。 */
function siteTitle(site: SourceTabItem): string {
  if (!site.supported) return `${site.name} · ${site.reason ?? '暂不可用'}`
  return site.note ? `${site.name} · ${site.note}` : site.name
}

/** 源健康禁用项的「重试」：清空该站点失败记录并重新加载全局健康快照。 */
async function retrySite(site: SourceTabItem) {
  if (api.demoMode) {
    ui.notify('演示模式不支持重试源健康', 'info')
    return
  }
  try {
    await api.sourceHealthReset('site', site.key)
    await config.loadHealth()
    if (siteDisabled(site.key)) {
      ui.notify(`源「${site.name}」仍处于禁用状态，请稍后再试`, 'warning')
      return
    }
    ui.notify(`已重置源「${site.name}」健康记录`, 'success')
    pickSite(site.key)
  } catch (e) {
    ui.notify('重置源健康记录失败：' + api.formatError(e), 'error')
  }
}

function pickSite(key: string) {
  if (!key) return
  if (key !== siteKey.value) config.setActiveSite('vod', key)
  siteDialogVisible.value = false
}

/** 弹窗内点击本机暂不可用的源：只提示原因，不切换 */
function onSiteBlocked(item: SourceTabItem) {
  ui.notify(item.reason || `${item.name} 需要后续支持（type=${item.type}）`, 'warning')
}

/** 多仓仓库切换（多仓时在站点弹窗内展示）；带上合集入口 sourceUrl 让后端保持 kind=multi */
async function onWarehouseSelect(url: string) {
  if (!url || url === config.enabled) return
  await config.setEnabled(url, config.sourceUrl || undefined)
}

function goSearch() {
  void router.push('/vod/search')
}

function goSettings() {
  void router.push('/settings')
}

/** 订阅源初始化失败后的手动重试 */
async function retryConfig() {
  await config.retryInit()
  if (!config.error) ui.notify('订阅源已重新加载', 'success')
}

function onSwitchSiteHint() {
  ui.notify(
    '请点击顶部「站点」按钮，选择本机可用的源（XML / 苹果CMS / HTTP 远程接口 / JS / 原生规则引擎 / Python）',
    'info',
  )
}

/* ---------------- 旧路由 /category/:site 的 ?site= 兼容 ---------------- */

/** 若 URL 带 ?site=<key> 且该站点存在，则覆盖当前站点并清除 query */
function consumeSiteQuery() {
  const raw = route.query.site
  const value = typeof raw === 'string' ? raw : Array.isArray(raw) ? raw[0] : ''
  if (!value) return
  let key = value
  try {
    key = decodeURIComponent(value)
  } catch {
    key = value
  }
  if (config.sites.some((s) => s.key === key)) config.setActiveSite('vod', key)
  // 清掉 query：刷新 / 返回时不再覆盖用户手动选择的站点
  void router.replace({ path: '/vod', query: {} })
}

watch(
  () => [config.initialized, config.enabled, siteKey.value, config.sites.length] as const,
  () => {
    if (!config.initialized) return
    consumeSiteQuery()
    ensureLoaded()
  },
  { immediate: true },
)

onMounted(() => {
  // 配置已在其它时机初始化完成时，挂载阶段立即处理 query 并加载
  if (config.initialized) {
    consumeSiteQuery()
    ensureLoaded()
  }
  // 「继续观看」区块依赖观看历史；未初始化时补一次加载
  if (!history.initialized) void history.load()
})

// 兼容 /category/:site 重定向：KeepAlive 缓存期间再次带 ?site= 进入时也能生效
watch(
  () => route.query.site,
  () => {
    if (!config.initialized) return
    consumeSiteQuery()
    ensureLoaded()
  },
)

/* ---------------- KeepAlive：缓存正在使用的数据 ---------------- */
// 滚动位置不在这里处理：内容区滚动由 default 布局按路由（fullPath）统一记住/恢复，
// 否则页面各自往共享滚动容器写 scrollTop，切到别的页面时会把位置“继承”过去。

onActivated(() => {
  // 从播放页 / 历史页返回时刷新「继续观看」（KeepAlive 缓存期间数据可能已变化）
  if (history.initialized) void history.load()
  // 设置页可能刚重置源健康：回到点播页时刷新禁用快照
  void config.loadHealth()
})
</script>

<template>
  <PageScroll>
    <div class="vod-page">
    <!-- ============ 顶部单行栏：首页 + 分类（横向滚动，flex:1） | 源切换 | 搜索 | 刷新 ============ -->
    <header class="vod-topbar head-sticky">
      <!-- 分类标签：首页 + 当前站点分类，多则横向滚动（el-scrollbar + 滚轮横向滚动），不折叠 -->
      <div
        class="cat-strip"
        role="group"
        aria-label="首页与当前站点分类"
        @wheel="onCatStripWheel"
      >
        <div
          v-if="catLoading && !categories.length"
          class="chips-skeleton"
          aria-label="分类加载中"
        >
          <span
            v-for="n in 8"
            :key="n"
            class="chip-skel"
          />
        </div>
        <StatusTabs
          :items="categoryTabs"
          :model-value="category"
          aria-label="首页与当前站点分类"
          @change="onCategorySelect"
        />
      </div>

      <!-- 源切换 + 工具条：右上角按钮组收进一个浮起胶囊，与左侧分类分段器同高 -->
      <div class="page-toolbar">
      <el-tooltip
        content="切换点播源"
        placement="bottom"
      >
        <button
          class="site-picker"
          type="button"
          aria-label="切换点播源"
          @click="openSiteDialog"
        >
          <SolarIcon
            class="site-picker-icon"
            name="monitor"
            size="1em"
          />
          <span class="site-picker-name">{{ currentSiteName || '选择源' }}</span>
          <SolarIcon
            class="site-picker-arrow"
            name="alt-arrow-down"
            :size="12"
          />
        </button>
      </el-tooltip>

      <!-- 搜索 / 刷新 -->
      <div class="site-btn">
        <el-tooltip
          content="搜索（快捷键 /）"
          placement="bottom"
        >
          <el-button
            circle
            aria-label="搜索"
            @click="goSearch"
          >
            <template #icon>
              <SolarIcon
                name="magnifier"
                size="1em"
              />
            </template>
          </el-button>
        </el-tooltip>
      </div>
      <div class="site-btn">
        <el-tooltip
          content="刷新"
          placement="bottom"
        >
          <el-button
            circle
            :loading="listLoading || catLoading || homeLoading"
            :disabled="!isHome && (!siteKey || siteUnsupported)"
            aria-label="刷新"
            @click="reload"
          >
            <template #icon>
              <SolarIcon
                name="refresh"
                size="1em"
              />
            </template>
          </el-button>
        </el-tooltip>
      </div>
      </div>
    </header>

    <!-- 站点选择弹窗（多仓时可在弹窗内先切换仓库） -->
    <el-dialog
      v-model="siteDialogVisible"
      title="选择点播站点"
      class="dlg-xl"
      append-to-body
      align-center
    >
      <div
        v-if="config.warehouseOptions.length"
        class="dlg-warehouse"
      >
        <span class="dlg-label">仓库</span>
        <button
          v-for="w in config.warehouseOptions"
          :key="w.url"
          type="button"
          class="dlg-chip"
          :class="{ 'is-active': w.active }"
          @click="onWarehouseSelect(w.url)"
        >
          {{ w.name }}
        </button>
      </div>

      <!-- 能力说明：本机源优先；不可用源按 REA 画像标注原因（待移植 / 需 Python / 不支持） -->
      <p class="dlg-hint">
        支持情况与 REA 站点画像联动：本机可用的源排在前面（XML / 苹果CMS / HTTP 远程接口 / JS spider /
        原生规则引擎 / 原生解析家族 / Python，需系统 Python）；画像判定为待移植的源会标注具体原因，
        鼠标悬停可查看完整说明。
      </p>

      <el-input
        v-model="siteKeyword"
        class="dlg-search"
        placeholder="搜索站点名称 / key"
        clearable
      >
        <template #prefix>
          <SolarIcon
            name="magnifier"
            size="1em"
          />
        </template>
      </el-input>

      <el-scrollbar
        class="site-scroll"
        max-height="360px"
      >
        <div
          class="site-list"
          aria-label="点播站点列表"
        >
          <button
            v-for="s in filteredSites"
            :key="s.key"
            type="button"
            class="site-item"
            :class="{ 'is-active': s.key === siteKey, 'is-disabled': !s.supported || siteDisabled(s.key) }"
            :aria-pressed="s.key === siteKey"
            :title="siteTitle(s)"
            @click="!s.supported ? onSiteBlocked(s) : siteDisabled(s.key) ? onSiteHealthBlocked(s) : pickSite(s.key)"
          >
            <span
              class="site-item-name"
              :title="s.name"
            >{{ s.name }}</span>
            <span
              v-if="s.supported && s.note"
              class="site-item-note"
              :title="s.note"
            >{{ s.note }}</span>
            <span
              v-if="!s.supported"
              class="site-item-badge"
              :title="s.reason"
            >
              {{ siteBadge(s) }}
            </span>
            <template v-else-if="siteDisabled(s.key)">
              <span class="site-item-badge site-item-badge--health">
                连续失败 {{ config.healthOf('site', s.key)?.failures ?? 3 }} 次
              </span>
              <span
                class="site-item-retry"
                role="button"
                tabindex="0"
                title="重置该站点失败记录并切换过去"
                @click.stop="retrySite(s)"
                @keydown.enter.stop="retrySite(s)"
              >重试</span>
            </template>
            <SolarIcon
              v-else-if="s.key === siteKey"
              class="site-item-check"
              name="check-circle"
              size="1em"
            />
          </button>
          <p
            v-if="!filteredSites.length"
            class="site-list-empty"
          >没有匹配的站点</p>
        </div>
      </el-scrollbar>
    </el-dialog>

    <!-- ============ 内容区 ============ -->
    <div class="vod-body">
      <!-- 演示数据提示条（mock 能力必须显式标注） -->
      <div
        v-if="config.demoMode"
        class="demo-hint"
      >
        <SolarIcon
          name="info-circle"
          size="1em"
        />
        <span>演示数据：当前为浏览器预览模式，内容来自 mock 适配层</span>
      </div>

      <!-- 首页：豆瓣热播（轮播 + 继续观看侧栏 + 豆瓣分类 + 卡片网格） -->
      <DoubanHome
        v-if="isHome"
        :home="home"
        :loading="homeLoading"
        :error="homeError"
        :with-side="continueItems.length > 0"
        @select="onDoubanSelect"
        @retry="loadHome(true)"
      >
        <template #side>
          <ContinueWatching
            :items="continueItems"
            @play="onContinuePlay"
          />
        </template>
      </DoubanHome>

      <!-- 源分类：原点播列表（四态 + 网格 + 分页） -->
      <template v-else>
        <!-- ① 初始化中 -->
        <SkeletonGrid
          v-if="initializing"
          :count="12"
        />

        <!-- ② 订阅配置读取失败（自动重试后仍失败，可手动重试） -->
        <ErrorState
          v-else-if="config.error"
          :message="config.error"
          title="订阅源加载失败"
          retry-text="重试"
          @retry="retryConfig"
        />

        <!-- ③ 无订阅 -->
        <EmptyState
          v-else-if="noSubscription"
          title="还没有订阅"
          desc="添加订阅源后即可浏览点播内容"
          action-text="去设置添加"
          @action="goSettings"
        />

        <!-- ④ 有订阅但未启用地址 -->
        <EmptyState
          v-else-if="noEnabled"
          title="已添加订阅，未启用任何地址"
          desc="在设置页选择一个地址启用后才能拉取站点"
          action-text="去选择地址"
          @action="goSettings"
        />

        <!-- ⑤ 启用源中没有站点 -->
        <EmptyState
          v-else-if="noSites"
          title="当前启用源没有点播站点"
          desc="可以更换订阅地址、在站点弹窗切换仓库/线路，或重新加载启用源"
          action-text="去设置"
          @action="goSettings"
        />

        <!-- ⑥ 源需要后续支持（jar / Python）或类型暂未适配 -->
        <ErrorState
          v-else-if="siteUnsupported"
          :message="unsupportedMessage"
          :title="unsupportedTitle"
          retry-text="换一个源"
          @retry="onSwitchSiteHint"
        />

        <!-- ⑦ 正常：网格 + 分页 -->
        <template v-else>
          <!-- 列表错误 -->
          <ErrorState
            v-if="error"
            :message="error"
            title="加载列表失败"
            retry-text="重试"
            @retry="retry"
          />

          <!-- 首次加载骨架 -->
          <SkeletonGrid
            v-else-if="listLoading && !items.length"
            :count="12"
          />

          <!-- 空列表：分类接口失败 / 该分类暂无内容 两种文案分开 -->
          <EmptyState
            v-else-if="!items.length"
            :title="emptyTitle"
            :desc="emptyDesc"
            :action-text="emptyActionText"
            @action="onEmptyAction"
          />

          <!-- 列表 + 分页 -->
          <template v-else>
            <VideoGrid
              :items="items"
              @select="onSelect"
            />
            <Pager
              :page="page"
              :has-next="hasNext"
              :loading="listLoading"
              @prev="onPrev"
              @next="onNext"
            />
          </template>
        </template>
      </template>
    </div>
    </div>
  </PageScroll>
</template>

<style scoped>
/* 页面根：统一 padding；顶部栏与内容之间用 margin-bottom 间距 */
.vod-page {
  display: flex;
  flex-direction: column;
  min-height: 100%;
  padding: var(--space-4);
}

.vod-page>* {
  margin-bottom: var(--space-4);
}

.vod-page>*:last-child {
  margin-bottom: 0;
}

/* —— 顶部单行栏：[首页 + 分类横向滚动（flex:1）] [源切换] [搜索] [刷新] ——
   属于内容区，与内容同底色；不用分割线，靠颜色层级区分 */
/* 顶部栏不再自绘底色/外扩：分段器与工具条各自是独立胶囊，阴影不该被滚动容器裁掉 */
.vod-topbar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

/* 源切换按钮：点击打开源选择弹窗（位于搜索图标左侧） */
.site-picker {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--space-1);
  max-width: 200px;
  height: 32px;
  padding: 0 10px;
  border: 0;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text);
  font: inherit;
  font-size: 13px;
  cursor: pointer;
  transition: var(--transition-color);
}

.site-picker:hover {
  background: var(--surface-2);
  color: var(--text);
}

.site-picker-icon {
  flex-shrink: 0;
  color: var(--accent);
}

.site-picker-name {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.site-picker-arrow {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}

/* 分类标签：占满剩余宽度，单行横向滚动（el-scrollbar，hover 显示细滚动条） */
.cat-strip {
  flex: 1;
  min-width: 0;
}

.cat-scrollbar {
  width: 100%;
  height: 44px;
}

/* 分类标签在 32px 高度内垂直居中（el-scrollbar 的滚动容器默认是块级） */
.cat-scrollbar :deep(.el-scrollbar__wrap.el-scrollbar__wrap--hidden-default) {
  display: flex;
  align-items: center;
}

.cat-scrollbar :deep(.status-tabs) {
  flex-wrap: nowrap;
}

/* 隐藏分类栏的滚动条（仍可横向滚动：滚轮 / 触控板 / 拖动标签） */
.cat-scrollbar :deep(.el-scrollbar__bar) {
  display: none;
}

.cat-strip :deep(.status-tab) {
  flex-shrink: 0;
}

.site-actions {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--space-1);
}

/* —— 站点选择弹窗 —— */
.dlg-warehouse {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.dlg-label {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.dlg-chip {
  padding: 4px 10px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  color: var(--text-regular);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: var(--transition-color);
}

.dlg-chip:hover {
  border-color: var(--accent);
  color: var(--accent);
}

.dlg-chip.is-active {
  border-color: var(--edge-accent);
  background: color-mix(in srgb, var(--el-color-primary) 10%, transparent);
  color: var(--el-color-primary);
  font-weight: 600;
}

.dlg-hint {
  margin: 0 0 var(--space-2);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--info-weak);
  color: var(--text-muted);
  font-size: 11.5px;
  line-height: 1.5;
}

.dlg-search {
  margin-bottom: var(--space-3);
}

.site-scroll {}

.site-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-2);
}

.site-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  padding: 8px 10px;
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
  color: var(--text);
  font: inherit;
  font-size: 13px;
  text-align: left;
  cursor: pointer;
  transition: var(--transition-color);
}

.site-item:hover {
  border-color: var(--accent);
  background: var(--accent-weak);
}

.site-item.is-active {
  border-color: var(--accent);
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

.site-item.is-disabled {
  background: var(--surface-2);
  color: var(--text-faint);
  cursor: not-allowed;
}

.site-item.is-disabled:hover {
  border-color: var(--border);
  background: var(--surface-2);
}

.site-item-name {
  flex: 1 1 auto;
  min-width: 4em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.site-item-badge--health {
  color: var(--danger);
  border-color: var(--danger);
}

.site-item-badge {
  flex: 0 1 auto;
  min-width: 0;
  max-width: 120px;
  overflow: hidden;
  font-size: 11px;
  color: var(--text-faint);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.site-item-retry {
  flex-shrink: 0;
  padding: 1px 8px;
  border: 0;
  background: var(--surface-2);
  border-radius: var(--radius-pill);
  color: var(--accent);
  font-size: 11px;
  line-height: 16px;
  cursor: pointer;
}

.site-item-retry:hover {
  border-color: var(--accent);
  background: var(--accent-weak);
}

.site-item-note {
  flex: 0 1 auto;
  min-width: 0;
  max-width: 120px;
  overflow: hidden;
  font-size: 10.5px;
  color: var(--text-faint);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.site-item-check {
  flex-shrink: 0;
  color: var(--accent);
}

.site-list-empty {
  grid-column: 1 / -1;
  margin: var(--space-4) 0;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}

/* —— 内容区：块与块之间统一 16px 间距 —— */
.vod-body {
  display: flex;
  flex-direction: column;
  padding: 0;
}

.vod-body>* {
  margin-bottom: var(--space-4);
}

.vod-body>*:last-child {
  margin-bottom: 0;
}

.demo-hint {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-3);
  font-size: 12px;
  color: var(--text-muted);
  background: var(--info-weak);
  border: 0;
  border-left: 3px solid var(--info);
  border-radius: var(--radius-md);
}

.demo-hint .solar-icon {
  color: var(--info);
}


.chips-skeleton {
  display: flex;
  gap: var(--space-2);
  overflow: hidden;
}

.chip-skel {
  flex-shrink: 0;
  width: 64px;
  height: 28px;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  animation: vod-pulse 1.2s ease-in-out infinite;
}

@keyframes vod-pulse {

  0%,
  100% {
    opacity: 0.55;
  }

  50% {
    opacity: 1;
  }
}

@media (prefers-reduced-motion: reduce) {
  .chip-skel {
    animation: none;
  }
}
</style>
