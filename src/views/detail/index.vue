<!-- 详情页（frontend-design.md §6.2 改版）：左上内嵌播放器窗格 + 右侧信息面板 + 下方整行选集
     - 点击任意一集：本页解析（useParseProgress + ParseProgressPanel）→ 直接 playerPane.open 播放，
       不再跳转 /player 路由；历史记录保留 upsert，离开页面时写回播放进度。
     - 视频画面由 libmpv 原生子窗口渲染（WebView2 下层），PlayerPane 负责上报位置与浮层 UI。
     - 解析失败 / 取消 / 重试逻辑沿用原有进度面板；不伪造播放成功。 -->
<script setup lang="ts">
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import * as api from '@/api'
import SolarIcon from '@/components/SolarIcon.vue'
import PlayerPane from '@/components/PlayerPane.vue'
import DetailHero from './components/DetailHero.vue'
import DownloadDialog from './components/DownloadDialog.vue'
import EpisodeLines from '@/components/EpisodeLines.vue'
import EpisodeGrid from '@/components/EpisodeGrid.vue'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import ParseProgressPanel from '@/components/ParseProgressPanel.vue'
import { useParseProgress } from '@/composables/useParseProgress'
import { useAutoSave } from '@/composables/useAutoSave'
import { usePerVideoAutoSave } from '@/composables/usePerVideoAutoSave'
import { useConfigStore } from '@/store/config'
import { useDownloadsStore } from '@/store/downloads'
import { useFavoritesStore } from '@/store/favorites'
import { useHistoryStore } from '@/store/history'
import { useUiStore } from '@/store/ui'
import { clearResolveCache, getResolveCache, resolveCacheKey, setResolveCache } from '@/utils/resolveCache'
import { detectCatvodErrorCode } from '@/utils/catvodErrors'
import { lineLabelAt } from '@/utils/episodeLines'
import { evictDetailView } from '@/layouts/vodDetailCache'
import type { DownloadFileInfo, Episode, VideoDetail } from '@/types'

defineOptions({ name: 'DetailView' })

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const downloads = useDownloadsStore()
const favorites = useFavoritesStore()
const history = useHistoryStore()
const ui = useUiStore()
/** 播放时自动保存（设置开启时，把当前集交给下载引擎再拉一份完整文件） */
const { autoSave } = useAutoSave()

/** 安全解码路由参数（key 可能含中文 / emoji） */
function safeDecode(value: unknown): string {
  const raw = typeof value === 'string' ? value : ''
  if (!raw) return ''
  try {
    return decodeURIComponent(raw)
  } catch {
    return raw
  }
}

const siteKey = computed(() => safeDecode(route.params.site))
const videoId = computed(() => safeDecode(route.params.id))
const siteName = computed(() => config.siteByKey(siteKey.value)?.name ?? siteKey.value)

const detail = ref<VideoDetail | null>(null)
const loading = ref(false)
const error = ref('')
const activeLine = ref(0)
const activeEp = ref(-1)
const resolving = ref(false)
/** 解析器选择（config.parses；空 = 后端默认解析器） */
const selectedParse = ref('')
/** 当前配置里的解析器列表（来自 LoadedConfig.config.parses） */
const parsers = computed(() => config.parsers)
/** 列表页经 history state 传来的分类名（仅用于元信息展示，不改路由 URL） */
const categoryName = ref('')

/** 本片「边下边播」覆盖开关：本片覆盖优先，否则跟随设置页全局「播放时自动保存」 */
const {
  effective: autoSaveEffective,
  override: autoSaveOverride,
  toggle: toggleAutoSave,
} = usePerVideoAutoSave(() => `${siteKey.value}/${detail.value?.id || videoId.value}`)

/* ---------------- 内嵌播放器 ---------------- */

/** 播放器窗格组件（解析成功后直接调用 open，离开页面时 close） */
const playerPane = ref<InstanceType<typeof PlayerPane> | null>(null)
/** 播放器列容器：用它的高度约束右侧信息面板，避免页面整体被撑高 */
const playerCol = ref<HTMLElement | null>(null)
/** 播放器窗格高度（px）：右侧信息区按它定高，内部简介再滚动 */
const paneHeight = ref(0)
let colObserver: ResizeObserver | null = null

/** 当前播放地址 / 标题 / 请求头（hasMedia 由 playingUrl 决定） */
const playingUrl = ref('')
const playingTitle = ref('')
const playingHeaders = ref<Record<string, string>>({})
/**
 * 当前播放的集坐标 + 起播时的影片元信息快照（写回历史用）。
 * 必须在起播瞬间把 siteKey/siteName/vodId/name 一起存下来：离开详情页时路由参数
 * 已经被切换到目标页面，再读 siteKey/siteName 会变成空串，导致写回的历史记录丢失
 * 站点信息（前端按「空站点记录不可打开」丢弃），表现为「历史进度不更新 / 不能续播」。
 */
const playing = ref<{
  gIndex: number
  epTitle: string
  index: number
  siteKey: string
  siteName: string
  line: string
  vodId: string
  name: string
  cover?: string
} | null>(null)
/** 当前播放集的保存信息（「保存本集」/自动保存都按站点 + 线路计算落盘目录） */
const playerSaveInfo = computed<DownloadFileInfo | null>(() => {
  const p = playing.value
  if (!p) return null
  return {
    siteKey: p.siteKey,
    siteName: p.siteName,
    line: p.line,
    vodId: p.vodId,
    epIndex: p.gIndex,
    name: p.name,
    epTitle: p.epTitle,
    cover: p.cover,
  }
})

/** 解析失败的文案（无媒体时由播放器窗格展示 error 态 + 重试） */
const parseErrorText = ref('')
/** 本次起播是否复用了「解析结果 TTL 缓存」：播放报错时要清掉缓存并重新解析 */
const playingFromCache = ref(false)
/** 本次起播是否在播放已下载的本地成品（重试时重新判定本地/在线，而不是重开同一路径） */
const playingLocal = ref(false)
/** 复用缓存时对应的 cache key（失败回退时精确清理） */
const playingCacheKey = ref('')

/** 右侧信息面板：与播放器窗格等高（内部简介滚动） */
const infoPanelStyle = computed(() =>
  paneHeight.value > 0 ? { height: `${paneHeight.value}px` } : undefined,
)

let reqSeq = 0
/** 本地成品查询进行中（毫秒级；防止缓存查询窗口内的重复点击各发一次解析） */
let localLookupBusy = false

/* ---------------- 解析进度（parse-progress 事件 → 选集右侧内联面板） ---------------- */

const {
  state: parseState,
  title: parseTitle,
  steps: parseSteps,
  elapsedMs: parseElapsedMs,
  stage: parseStage,
  errorMessage: parseError,
  visible: parseVisible,
  currentStep: parseCurrentStep,
  begin: beginParse,
  finish: finishParse,
  cancel: cancelParseRun,
  close: closeParse,
  reopen: reopenParse,
} = useParseProgress()

/** 网盘未登录：解析失败后弹一次「去登录」引导（W13 错误联动；同一错误文案只弹一次） */
let panPromptedFor = ''
watch([parseState, parseError], ([state, message]) => {
  if (state !== 'error' || !message || message === panPromptedFor) return
  const needsLogin =
    detectCatvodErrorCode(message) === 'catvod.pan.cookie_required' ||
    /login_required|需要登录/.test(message)
  if (!needsLogin) return
  panPromptedFor = message
  void ElMessageBox.confirm('该资源需要登录网盘账号才能播放。是否现在去登录？', '需要登录网盘', {
    confirmButtonText: '去登录',
    cancelButtonText: '稍后',
    type: 'warning',
  })
    .then(() => router.push({ path: '/cloud', query: { accounts: '1' } }))
    .catch(() => undefined)
})

/** 选集右侧「解析过程」：在显示 / 隐藏之间切换（面板内联在和选集并排的右列） */
function onToggleParseLog() {
  if (parseVisible.value) closeParse()
  else reopenParse()
}

/** 最近一次发起解析的集：面板「重试」用它重新解析同一集 */
const lastAttempt = ref<{ ep: Episode; index: number } | null>(null)
/** 用户是否点过「取消解析」：仅用于把收尾提示从「失败」改成「已取消」 */
const cancelRequested = ref(false)

/** 播放器窗格 opening 文案（解析中显示当前步骤；复用解析缓存时直接提示打开播放器） */
const paneLoadingText = computed(() => {
  if (!resolving.value) return ''
  if (playingFromCache.value) return '复用上次解析结果，正在打开播放器…'
  return `正在解析：${parseCurrentStep.value?.title || '等待后端返回'}`
})

/** 面板「取消解析」：尽力而为，最终结果仍以 resolve/sniff 返回为准 */
function onParseCancel() {
  cancelRequested.value = true
  void cancelParseRun()
}

/** 面板「重试」：重新发起同一集的解析（resolving 守卫已复位） */
function onParseRetry() {
  const attempt = lastAttempt.value
  if (!attempt) return
  void playEpisode(attempt.ep, attempt.index)
}

function readCategoryState() {
  const state = window.history.state as Record<string, unknown> | null
  const value = state?.vodCategory
  categoryName.value = typeof value === 'string' ? value : ''
}

/** 只保留有内容的线路（后端可能返回空线路组） */
const lines = computed(() => (detail.value?.episodes ?? []).filter((g) => g.items.length > 0))
const currentLine = computed(() => lines.value[activeLine.value])
const episodes = computed<Episode[]>(() => currentLine.value?.items ?? [])
const hasEpisodes = computed(() => lines.value.length > 0)

/** 跨线路的全局集序号（0 起始），历史记录 epIndex 用该值 */
function globalEpisodeIndex(lineIndex: number, index: number): number {
  let offset = 0
  for (let i = 0; i < lineIndex; i += 1) offset += lines.value[i]?.items.length ?? 0
  return offset + index
}

/** 按全局集序号定位到具体线路 + 集（历史记录跳转用） */
function locateEpisode(gIndex: number): { line: number; index: number; ep: Episode } | null {
  for (let line = 0; line < lines.value.length; line += 1) {
    const items = lines.value[line]?.items ?? []
    const offset = globalEpisodeIndex(line, 0)
    if (gIndex >= offset && gIndex < offset + items.length) {
      return { line, index: gIndex - offset, ep: items[gIndex - offset] }
    }
  }
  return null
}

/* ---------------- 自动起播：历史 query 优先，否则第一条线路第一集 ---------------- */

/** ?ep=全局集序号&t=已看秒数（由历史记录页带上） */
function resumeQueryTarget(): { gIndex: number; startSec: number } | null {
  const epRaw = route.query.ep
  if (typeof epRaw !== 'string' || epRaw.trim() === '') return null
  const gIndex = Number(epRaw)
  if (!Number.isFinite(gIndex) || gIndex < 0) return null
  const tRaw = route.query.t
  const t = typeof tRaw === 'string' ? Number(tRaw) : 0
  return {
    gIndex: Math.floor(gIndex),
    startSec: Number.isFinite(t) && t > 5 ? Math.floor(t) : 0,
  }
}

/** 同一次进入（同一影片）只自动起播一次（query 清理 / 刷新详情 / 重渲染都不重复） */
let autoStartedKey = ''

/**
 * 详情加载完成且有可用选集后自动起播一次：
 * - 有历史 query 且能定位到全局集序号 → 选中该集并带进度起播；
 * - 无 query / 定位失败（集数越界）→ 自动选中第一条线路第一集起播；
 * - 换影片（路由参数变化）后 key 不同，允许再次自动起播；刷新按钮加载同一影片不重复。
 */
watch(
  () => [detail.value?.id, loading.value, hasEpisodes.value] as const,
  async () => {
    const target = detail.value
    if (!target?.id || loading.value || !hasEpisodes.value) return
    const mediaKey = `${siteKey.value}/${target.id || videoId.value}`
    if (autoStartedKey === mediaKey) return
    autoStartedKey = mediaKey
    // 历史 query：起播前清掉 ep/t，避免返回详情页 / 刷新时重复自动播放
    const resume = resumeQueryTarget()
    if (resume) void router.replace({ query: { ...route.query, ep: undefined, t: undefined } })
    const found = resume ? locateEpisode(resume.gIndex) : null
    if (found && resume) {
      activeLine.value = found.line
      activeEp.value = found.index
      await nextTick()
      await playEpisode(found.ep, found.index, resume.startSec)
      return
    }
    // 无历史 query 或定位失败：回退第一条线路第一集
    const first = lines.value[0]?.items[0]
    if (!first) return
    activeLine.value = 0
    activeEp.value = 0
    await nextTick()
    await playEpisode(first, 0)
  },
)

/* ---------------- 详情加载 ---------------- */

/** 上一次加载的影片 key：路由参数变化时先复位正在播放的媒体 */
let lastMediaKey = ''

/** 关掉内嵌播放器并复位播放相关状态 */
function resetPlayback() {
  const pane = playerPane.value
  if (pane) {
    void pane.close()
  }
  playingUrl.value = ''
  playingTitle.value = ''
  playingHeaders.value = {}
  playing.value = null
  playingFromCache.value = false
  playingLocal.value = false
  playingCacheKey.value = ''
  parseErrorText.value = ''
}

async function loadDetail() {
  if (!siteKey.value || !videoId.value) {
    detail.value = null
    loading.value = false
    error.value = '缺少站点或资源参数，无法加载详情'
    return
  }
  const mediaKey = `${siteKey.value}/${videoId.value}`
  if (mediaKey !== lastMediaKey) {
    // 同一组件内直接切换影片（不走路由卸载）时也要先写回上一部的进度
    if (lastMediaKey) writebackProgress()
    resetPlayback()
    lastMediaKey = mediaKey
  }
  const seq = ++reqSeq
  loading.value = true
  error.value = ''
  detail.value = null
  activeLine.value = 0
  activeEp.value = -1
  try {
    const res = await api.getDetail(siteKey.value, videoId.value)
    if (seq !== reqSeq) return
    if (!res || !res.id) {
      error.value = '详情为空：后端未返回有效数据'
      return
    }
    detail.value = res
  } catch (e) {
    if (seq !== reqSeq) return
    error.value = api.formatError(e)
  } finally {
    if (seq === reqSeq) loading.value = false
  }
}

watch(
  () => [
    siteKey.value,
    videoId.value,
    config.initialized,
    config.activeSiteKey('vod'),
  ] as const,
  () => {
    // 离开详情页时 route.params 会先变空，不能把已有的详情清掉
    if (!config.initialized || !siteKey.value || !videoId.value) return
    // 从搜索/设置切回同一部影片时复用 KeepAlive 中的详情，避免重新加载
    const mediaKey = `${siteKey.value}/${videoId.value}`
    if (detail.value?.id && lastMediaKey === mediaKey) return
    readCategoryState()
    void loadDetail()
  },
  { immediate: true },
)

/* ---------------- 右侧信息面板高度 = 播放器窗格高度 ---------------- */

watch(
  playerCol,
  (el) => {
    colObserver?.disconnect()
    colObserver = null
    paneHeight.value = 0
    if (!el) return
    paneHeight.value = Math.round(el.getBoundingClientRect().height)
    if (typeof ResizeObserver === 'undefined') return
    colObserver = new ResizeObserver((entries) => {
      const height = entries[0]?.contentRect.height ?? 0
      // 小窗态下列容器会塌陷为 0：保留上一次的有效高度，避免右侧信息面板跟着收起
      if (height > 0) paneHeight.value = Math.round(height)
    })
    colObserver.observe(el)
  },
  { flush: 'post' },
)

/** 播放中定时写回进度的间隔：直接关窗 / 崩溃时 onBeforeUnmount 不一定触发，最多丢 15s */
const PROGRESS_WRITEBACK_MS = 15_000
let progressTimer: number | null = null

/** 播放进度定时器可被激活/停用复用：详情页所在的一级布局会被 KeepAlive 缓存 */
function startProgressTimer() {
  if (progressTimer !== null) return
  progressTimer = window.setInterval(() => {
    if (playing.value) writebackProgress()
  }, PROGRESS_WRITEBACK_MS)
}

function stopProgressTimer() {
  if (progressTimer === null) return
  window.clearInterval(progressTimer)
  progressTimer = null
}

onMounted(() => {
  // 滚动位置由 default 布局按路由（fullPath）统一记住/恢复；详情页首次进入自然在顶部
  readCategoryState()
  // 历史进度用于「继续观看」起播位置；未初始化时补一次加载
  if (!history.initialized) void history.load()
  // 收藏列表用于「已收藏」按钮状态；未初始化时补一次加载
  if (!favorites.initialized) void favorites.load()
  // 下载设置（「播放时自动保存」开关）在播放前读一次
  if (!downloads.settingsLoaded) void downloads.loadSettings()
  // 播放中定时落一次进度：避免用户直接关窗口时历史进度丢失
  startProgressTimer()
  window.addEventListener('beforeunload', writebackProgress)
})

// 一级布局被 KeepAlive 缓存：切走时停掉播放/定时器，回来时不保留后台播放
const DETAIL_ROUTE_NAMES = new Set(['vod-detail', 'history-detail', 'favorites-detail'])
// 同一个 DetailView 会被点播/历史/收藏三个布局各自缓存；记录本实例创建时的
// 详情路由名，避免历史详情在被缓存时因路由切到点播详情而误启动定时器。
const detailRouteName = String(route.name ?? '')
const isOwnDetailRoute = () => String(route.name) === detailRouteName

onDeactivated(() => {
  writebackProgress()
  stopProgressTimer()
  resetPlayback()
})

onActivated(() => {
  if (isOwnDetailRoute()) startProgressTimer()
})

/**
 * 外层布局被默认布局 KeepAlive 缓存时，子组件不会触发 onDeactivated；
 * 这里直接监听路由，离开详情页（切设置/历史/搜索）就停播并复位，避免后台继续播放。
 * 回到详情页时重新启动进度定时器（此时 onActivated 也不一定触发）。
 */
watch(
  () => route.fullPath,
  () => {
    if (isOwnDetailRoute() && DETAIL_ROUTE_NAMES.has(detailRouteName)) {
      startProgressTimer()
      return
    }
    writebackProgress()
    stopProgressTimer()
    resetPlayback()
  },
  // 同步执行：路由参数一变就先停播，避免布局被 KeepAlive 移走后再 close 时播放器已不可用
  { flush: 'sync' },
)

onBeforeUnmount(() => {
  writebackProgress()
  stopProgressTimer()
  resetPlayback()
  window.removeEventListener('beforeunload', writebackProgress)
  colObserver?.disconnect()
  colObserver = null
  writebackProgress()
  void playerPane.value?.close()
})

/* ---------------- 交互 ---------------- */

function onLineSelect(index: number) {
  if (index === activeLine.value) return
  activeLine.value = index
  activeEp.value = -1
}

/** 切换解析器：同步给后端默认解析器（之后 resolve 未显式传 parseName 时生效） */
async function onParseChange() {
  const name = selectedParse.value
  if (!name) return
  try {
    await api.setDefaultParse(name)
    ui.notify(`默认解析器：${name}`, 'success')
  } catch (e) {
    ui.notify('设置默认解析器失败：' + api.formatError(e), 'error')
  }
}

/** 从历史记录取起播位置（仅同一影片同一集，且进度有效时） */
function resumeStartSec(gIndex: number): number {
  const item = history.items.find(
    (it) =>
      it.siteKey === siteKey.value &&
      it.id === (detail.value?.id || videoId.value) &&
      it.epIndex === gIndex,
  )
  if (!item) return 0
  const pos = item.positionSec || 0
  if (pos <= 5) return 0
  if (item.durationSec > 0 && pos >= item.durationSec - 10) return 0
  return Math.floor(pos)
}

/**
 * 写回当前播放进度到历史（离开页面 / 切换影片时调用）。
 * 注意：一律使用 playing 里的元信息快照，不能在离开时再读 siteKey/siteName ——
 * 此时路由已切到目标页面，route.params 为空，会写出「空站点」记录而被历史列表丢弃。
 */
function writebackProgress() {
  const played = playing.value
  const paneState = playerPane.value?.getState()
  if (!played || !paneState || !paneState.url) return
  const positionSec = Math.round(paneState.positionSec || 0)
  if (positionSec <= 0) return
  void history.upsert({
    siteKey: played.siteKey,
    siteName: played.siteName,
    id: played.vodId,
    name: played.name,
    cover: played.cover,
    epIndex: played.gIndex,
    epTitle: played.epTitle,
    positionSec,
    durationSec: Math.round(paneState.durationSec || 0),
    updatedAt: Date.now(),
  })
}

/**
 * 该集是否已有可用的本地成品（下载 / 边看边存导出）。
 * 后端校验「completed 且文件仍存在」；命中返回绝对路径，供直接本地播放，
 * 避免已下载的集从历史记录点进来又走一遍在线解析 + HLS 缓存。
 */
async function findLocalPlaybackFile(
  target: VideoDetail,
  gIndex: number,
  epTitle: string,
  line: string,
): Promise<string | null> {
  try {
    const task = await api.downloadLocalFile({
      siteKey: siteKey.value,
      siteName: siteName.value,
      line,
      vodId: target.id || videoId.value,
      epIndex: gIndex,
      name: target.name,
      epTitle,
      cover: target.cover ?? undefined,
    })
    return task?.filePath || null
  } catch {
    return null
  }
}

/**
 * 点击集：解析直链（resolve + 需要时 sniff）→ 记录历史 → 直接在内嵌播放器窗格播放。
 * 解析/抓包全程通过 parse-progress 事件推进选集右侧（6:4）的内联进度面板；requestId 在 invoke 之前生成，
 * 保证后端第一批事件不丢。解析失败保留进度面板（含重试），不伪造播放成功。
 */
async function playEpisode(
  ep: Episode,
  index: number,
  startOverride?: number,
  fallbackUsed = false,
) {
  const target = detail.value
  if (!target || resolving.value) return
  activeEp.value = index
  /** 解析失败后要自动切换的线路（finally 后重试，避免 resolving 守卫拦截） */
  let fallback: { line: number; index: number; ep: Episode } | null = null

  const gIndex = globalEpisodeIndex(activeLine.value, index)
  const epTitle = ep.title || `第${index + 1}集`
  const line = lineLabelAt(lines.value, activeLine.value)
  const startSec = startOverride ?? resumeStartSec(gIndex)

  // 0) 该集已有本地成品（下载 / 边看边存）：直接播本地文件，跳过在线解析与 HLS 缓存。
  //    历史续播点进来时尤其重要：已下载的集不应再走一遍 resolve + 新缓存。
  if (localLookupBusy) return
  localLookupBusy = true
  let localPath: string | null = null
  try {
    localPath = await findLocalPlaybackFile(target, gIndex, epTitle, line)
  } finally {
    localLookupBusy = false
  }
  if (localPath) {
    lastAttempt.value = { ep, index }
    cancelRequested.value = false
    parseErrorText.value = ''
    playingLocal.value = true
    playingFromCache.value = false
    playingCacheKey.value = ''
    playingUrl.value = localPath
    playingHeaders.value = {}
    playingTitle.value = epTitle
    playing.value = {
      gIndex,
      epTitle,
      index,
      siteKey: siteKey.value,
      siteName: siteName.value,
      line,
      vodId: target.id || videoId.value,
      name: target.name,
      cover: target.cover ?? undefined,
    }
    await history.upsert({
      siteKey: siteKey.value,
      siteName: siteName.value,
      id: target.id || videoId.value,
      name: target.name,
      cover: target.cover ?? undefined,
      epIndex: gIndex,
      epTitle,
      positionSec: startSec,
      durationSec: 0,
      updatedAt: Date.now(),
    })
    await nextTick()
    try {
      await playerPane.value?.open(localPath, {}, startSec)
      ui.notify(`播放本地已下载文件：${epTitle}`, 'success')
      return
    } catch (openError) {
      // 本地文件打不开（被删 / 损坏 / 编码不支持）：回退在线解析，不影响观看
      playingLocal.value = false
      ui.notify('本地文件播放失败，改为在线解析：' + api.formatError(openError), 'warning')
    }
  }

  if (!ep?.url) {
    ui.notify('该集缺少播放地址，无法解析', 'warning')
    return
  }

  const cacheKey = resolveCacheKey({
    siteKey: siteKey.value,
    vodId: target.id || videoId.value,
    line: currentLine.value?.group ?? '',
    epIndex: gIndex,
    parseName: selectedParse.value || '',
    epUrl: ep.url,
  })
  lastAttempt.value = { ep, index }
  cancelRequested.value = false
  resolving.value = true
  parseErrorText.value = ''
  playingFromCache.value = false
  playingCacheKey.value = ''
  try {
    // 解析结果 TTL 缓存：同一集 30 分钟内再次进入直接复用上次的直链 + 请求头，跳过 resolve/抓包
    const cached = getResolveCache(cacheKey)
    let playUrl = ''
    let headers: Record<string, string> = {}
    let usedCache = false
    if (cached) {
      playUrl = cached.url
      headers = cached.headers
      usedCache = true
      playingFromCache.value = true
      playingCacheKey.value = cacheKey
    } else {
      // begin() 必须在真正 invoke 之前：否则后端最先发出的进度事件会被过滤掉
      const requestId = beginParse({ title: ep.title || `第${index + 1}集` })
      const first = await api.resolveSource({
        site: siteKey.value,
        flag: currentLine.value?.group,
        url: ep.url,
        id: target.id || videoId.value,
        parseName: selectedParse.value || undefined,
        requestId,
      })

      // 后端标记 sniff=true：交给桌面端 WebView 抓包拿真实直链
      playUrl = first.url
      headers = first.headers ?? {}
      let sniffed = false
      if (first.sniff) {
        const sniffedResult = await api.sniff({
          url: first.url,
          headers: first.headers,
          click: config.siteByKey(siteKey.value)?.click || undefined,
          // 同一次解析沿用同一个 requestId，进度事件才能继续落到本面板
          requestId,
        })
        playUrl = sniffedResult.url || first.url
        headers = sniffedResult.headers ?? headers
        sniffed = true
      }

      if (!playUrl) {
        const message = '解析完成但未返回播放地址'
        parseErrorText.value = message
        finishParse('error', message)
        ui.notify(message, 'warning')
        return
      }

      finishParse('success')

      // 记住这次解析结果（含抓包后的最终地址 + 请求头）：TTL 内同集直接复用，失败时清掉重解析
      setResolveCache(cacheKey, playUrl, headers)

      const meta = [
        first.parseName ? `解析器 ${first.parseName}` : '',
        first.jxFrom ? `来源 ${first.jxFrom}` : '',
        sniffed ? '已抓包' : '',
      ]
        .filter(Boolean)
        .join(' · ')
      ui.notify(`已解析到直链${meta ? `（${meta}）` : ''}，正在打开播放器`, 'success')
    }

    playingLocal.value = false
    playingUrl.value = playUrl
    playingHeaders.value = headers
    playingTitle.value = epTitle
    playing.value = {
      gIndex,
      epTitle,
      index,
      // 快照：离开页面时路由已切走，siteKey/siteName 会变空，写回历史必须用这份值
      siteKey: siteKey.value,
      siteName: siteName.value,
      line,
      vodId: target.id || videoId.value,
      name: target.name,
      cover: target.cover ?? undefined,
    }

    await history.upsert({
      siteKey: siteKey.value,
      siteName: siteName.value,
      id: target.id || videoId.value,
      name: target.name,
      cover: target.cover ?? undefined,
      epIndex: gIndex,
      epTitle,
      positionSec: startSec,
      durationSec: 0,
      updatedAt: Date.now(),
    })

    // 详情页内直接播放（不再 router.push 到 /player）；后端会 replace 当前媒体
    await nextTick()
    try {
      await playerPane.value?.open(playUrl, headers, startSec)
    } catch (openError) {
      // 复用的缓存地址可能已过期 / 被防盗链拒绝：清掉该条，等 resolving 复位后再重新解析一次
      if (!usedCache) throw openError
      clearResolveCache(cacheKey)
      playingFromCache.value = false
      playingCacheKey.value = ''
      ui.notify('缓存地址已失效，正在重新解析…', 'info')
      window.setTimeout(() => {
        void playEpisode(ep, index, startOverride, fallbackUsed)
      }, 0)
      return
    }

    // 播放时自动保存：等播放器打开后再登记（HLS 会走本地代理导出，避免重复拉流）
    // 本片覆盖优先：undefined = 未覆盖（沿用全局开关 + 等设置加载），true/false = 本片强制开/关
    void autoSave(
      {
        siteKey: siteKey.value,
        siteName: siteName.value,
        line,
        vodId: target.id || videoId.value,
        epIndex: gIndex,
        name: target.name,
        epTitle,
        cover: target.cover ?? undefined,
        url: playUrl,
        headers,
      },
      autoSaveOverride.value,
    )
  } catch (e) {
    // 失败既 notify 又保留面板（面板里是完整解析日志，方便排障）
    const message = api.formatError(e)
    parseErrorText.value = message
    finishParse('error', message)
    if (cancelRequested.value) {
      ui.notify('已取消解析', 'info')
    } else {
      ui.notify('解析失败：' + message, 'error')
      // 记录当前线路失败（源健康度），并准备自动换线路重试一次
      const lineKey = config.enabled || config.loaded?.activeLine || ''
      if (lineKey) void api.sourceHealthRecord('line', lineKey, config.activeLineName, false, message)
      if (!fallbackUsed) fallback = pickFallbackLine(index, ep.title)
    }
  } finally {
    resolving.value = false
  }
  if (fallback) {
    const lineName = lines.value[fallback.line]?.group || `线路 ${fallback.line + 1}`
    ui.notify(`当前线路解析失败，已自动切换到「${lineName}」重试`, 'warning')
    activeLine.value = fallback.line
    void playEpisode(fallback.ep, fallback.index, startOverride, true)
  }
}

/**
 * 解析失败后的备用线路：优先同集下标，退化到同集标题；仅取第一条有播放地址的线路。
 * 返回 null 表示没有其它可用线路（保持原失败提示）。
 */
function pickFallbackLine(
  index: number,
  title: string,
): { line: number; index: number; ep: Episode } | null {
  const current = activeLine.value
  for (let line = 0; line < lines.value.length; line += 1) {
    if (line === current) continue
    const items = lines.value[line]?.items ?? []
    if (!items.length) continue
    const same = items[index] ?? items.find((item) => (item.title || '') === (title || ''))
    if (!same?.url) continue
    return { line, index: items.indexOf(same), ep: same }
  }
  return null
}

/** EpisodeGrid 的 play 事件（ep + 当前线路内下标） */
function onEpisodePlay(ep: Episode, index: number) {
  void playEpisode(ep, index)
}

/* ---------------- 收藏 ---------------- */

/** 收藏操作进行中（按钮 loading，避免重复提交） */
const favoriteLoading = ref(false)

/** 当前影片是否已收藏（详情加载前用路由 id 兜底，加载后以详情 id 为准） */
const favorited = computed(() => favorites.isFavorite(siteKey.value, detail.value?.id || videoId.value))

/** 收藏 / 取消收藏：成功后同步 store 列表并提示 */
async function onToggleFavorite() {
  const target = detail.value
  if (!target || favoriteLoading.value) return
  favoriteLoading.value = true
  try {
    const nowFavorited = await favorites.toggle({
      siteKey: siteKey.value,
      siteName: siteName.value,
      vodId: target.id || videoId.value,
      name: target.name,
      cover: target.cover ?? undefined,
      year: target.year ?? undefined,
      area: target.area ?? undefined,
      remark: target.desc ? target.desc.slice(0, 100) : undefined,
      createdAt: Date.now(),
    })
    ui.notify(nowFavorited ? '已收藏' : '已取消收藏', 'success')
  } catch (e) {
    ui.notify('收藏操作失败：' + api.formatError(e), 'error')
  } finally {
    favoriteLoading.value = false
  }
}

/* ---------------- 下载：解析直链 → 交给 Rust 下载引擎 ---------------- */

/** 「下载」/「下载全部」按钮 loading（互斥，避免同时大量解析） */
const downloading = ref(false)
const downloadingAll = ref(false)
/** 下载线路/范围选择对话框 */
const downloadDialogOpen = ref(false)
const downloadDialogDefaultScope = ref<'current' | 'all'>('current')
/** 当前播放/选中集在当前线路内的下标（对话框「当前集」默认目标） */
const downloadActiveIndex = computed(() => {
  const played = playing.value?.index ?? -1
  if (played >= 0) return played
  return Math.max(activeEp.value, 0)
})

/**
 * 静默解析一集直链（resolve + 按需 sniff）。
 * 使用独立 requestId、不占用播放解析状态（resolving / 进度面板）：
 * 下载不应打断正在播放或正在解析的剧集。
 */
async function resolveForDownload(
  ep: Episode,
  index: number,
  lineIndex = activeLine.value,
): Promise<{ url: string; headers: Record<string, string> }> {
  const target = detail.value
  if (!target) throw new Error('详情尚未加载')
  if (!ep?.url) throw new Error('该集缺少播放地址')
  // 独立 requestId：进度事件不会串到播放解析面板
  const requestId = `dl-${Date.now().toString(36)}-${lineIndex}-${index}`
  const first = await api.resolveSource({
    site: siteKey.value,
    flag: lines.value[lineIndex]?.group,
    url: ep.url,
    id: target.id || videoId.value,
    parseName: selectedParse.value || undefined,
    requestId,
  })
  let url = first.url
  let headers = first.headers ?? {}
  if (first.sniff) {
    const sniffed = await api.sniff({
      url: first.url,
      headers: first.headers,
      click: config.siteByKey(siteKey.value)?.click || undefined,
      requestId,
    })
    url = sniffed.url || first.url
    headers = sniffed.headers ?? headers
  }
  if (!url) throw new Error('解析完成但未返回直链')
  return { url, headers }
}

/** 打开「选择下载线路」对话框：线路通常对应不同清晰度/速度 */
function openDownloadDialog(scope: 'current' | 'all') {
  if (downloading.value || downloadingAll.value) return
  if (!detail.value || !lines.value.length) {
    ui.notify('该资源暂无可下载的选集', 'warning')
    return
  }
  downloadDialogDefaultScope.value = scope
  downloadDialogOpen.value = true
}

/** 「下载」：先选线路，再解析当前集加入队列 */
function onDownload() {
  openDownloadDialog('current')
}

/** 「下载全部」：先选线路，再解析该线路全部选集加入队列 */
function onDownloadAll() {
  openDownloadDialog('all')
}

/** 下载对话框确认：按选中的线路 + 范围解析并入队 */
async function onDownloadConfirm(payload: { lineIndex: number; scope: 'current' | 'all' }) {
  const target = detail.value
  const group = lines.value[payload.lineIndex]
  if (!target || !group || !group.items.length) {
    ui.notify('该线路暂无可下载的选集', 'warning')
    return
  }
  const lineIndex = payload.lineIndex
  const line = lineLabelAt(lines.value, lineIndex)

  if (payload.scope === 'current') {
    downloading.value = true
    try {
      const index = Math.min(Math.max(downloadActiveIndex.value, 0), group.items.length - 1)
      const ep = group.items[index]
      const { url, headers } = await resolveForDownload(ep, index, lineIndex)
      const epTitle = ep.title || `第${index + 1}集`
      await downloads.start({
        siteKey: siteKey.value,
        siteName: siteName.value,
        line,
        vodId: target.id || videoId.value,
        epIndex: globalEpisodeIndex(lineIndex, index),
        name: target.name,
        epTitle,
        cover: target.cover ?? undefined,
        url,
        headers,
      })
      ui.notify(`已加入下载队列：${epTitle}`, 'success')
      downloadDialogOpen.value = false
    } catch (e) {
      ui.notify('加入下载失败：' + api.formatError(e), 'error')
    } finally {
      downloading.value = false
    }
    return
  }

  // 本线路全部：串行解析，单集失败继续并汇总
  const list = [...group.items]
  downloadingAll.value = true
  let added = 0
  let failed = 0
  ui.notify(`正在解析并加入 ${list.length} 集下载任务…`, 'info')
  try {
    for (let i = 0; i < list.length; i += 1) {
      const ep = list[i]
      try {
        const { url, headers } = await resolveForDownload(ep, i, lineIndex)
        const epTitle = ep.title || `第${i + 1}集`
        await downloads.start({
          siteKey: siteKey.value,
          siteName: siteName.value,
          line,
          vodId: target.id || videoId.value,
          epIndex: globalEpisodeIndex(lineIndex, i),
          name: target.name,
          epTitle,
          cover: target.cover ?? undefined,
          url,
          headers,
        })
        added += 1
      } catch {
        // 单集失败不影响后续；最终统一汇总提示
        failed += 1
      }
    }
  } finally {
    downloadingAll.value = false
    downloadDialogOpen.value = false
  }
  if (added > 0 && failed > 0) {
    ui.notify(`已加入 ${added} 个下载任务，${failed} 集解析失败`, 'warning')
  } else if (added > 0) {
    ui.notify(`已加入 ${added} 个下载任务`, 'success')
  } else {
    ui.notify(`全部 ${failed} 集解析失败，未加入下载任务`, 'error')
  }
}

/** 播放器「选集」弹层：按线路内下标播放 */
function playEpisodeAt(index: number) {
  const ep = episodes.value[index]
  if (ep) void playEpisode(ep, index)
}

/** 播放器「上一集」 */
function playPrevEpisode() {
  const index = playing.value?.index ?? -1
  if (index > 0) playEpisodeAt(index - 1)
}

/** 播放器「下一集」 */
function playNextEpisode() {
  const index = playing.value?.index ?? -1
  if (index >= 0 && index < episodes.value.length - 1) playEpisodeAt(index + 1)
}

/** [▶ 播放第一集]（沿用按钮，直接在窗格内播放第一集） */
function playFirst() {
  const first = episodes.value[0]
  if (!first) {
    ui.notify('该资源暂无可播放线路', 'warning')
    return
  }
  void playEpisode(first, 0)
}

/* ---------------- 单片「边下边播」开关 ---------------- */

/** 立即对当前正在播放的集触发一次自动保存（复用起播时保存的影片快照） */
async function autoSaveCurrentEpisode() {
  const played = playing.value
  if (!played || !playingUrl.value) return
  await autoSave(
    {
      siteKey: played.siteKey,
      siteName: played.siteName,
      line: played.line,
      vodId: played.vodId,
      epIndex: played.gIndex,
      name: played.name,
      epTitle: played.epTitle,
      cover: played.cover,
      url: playingUrl.value,
      headers: playingHeaders.value,
    },
    true,
  )
}

/** 控制条「边下边播」：翻转本片覆盖值并持久化；播放中从关→开时立即补一次自动保存 */
function onToggleAutoSave() {
  const enabled = toggleAutoSave()
  ui.notify(`本片边下边播：${enabled ? '已开启' : '已关闭'}`, enabled ? 'success' : 'info')
  if (enabled) void autoSaveCurrentEpisode()
}

/**
 * 播放器窗格「重试 / 重播」：
 * - 本次起播用的是缓存地址：清掉缓存并重新解析（旧地址可能已失效）；
 * - 否则优先重开当前直链，最后才重新解析最近一集。
 */
function onPlayerRetry() {
  // 本地文件播放失败（被删 / 损坏）：重新判定该集——本地仍可用就重开，否则自动回退在线解析
  if (playingLocal.value) {
    const attempt = lastAttempt.value
    if (attempt) {
      void playEpisode(attempt.ep, attempt.index)
      return
    }
  }
  if (playingFromCache.value) {
    const attempt = lastAttempt.value
    clearResolveCache(playingCacheKey.value)
    playingFromCache.value = false
    playingCacheKey.value = ''
    if (attempt) {
      ui.notify('正在重新解析该集…', 'info')
      void playEpisode(attempt.ep, attempt.index)
      return
    }
  }
  if (playingUrl.value) {
    void playerPane.value?.open(playingUrl.value, playingHeaders.value)
    return
  }
  const attempt = lastAttempt.value
  if (attempt) {
    void playEpisode(attempt.ep, attempt.index)
    return
  }
  ui.notify('暂无可重试的剧集，请重新选择', 'info')
}

/**
 * 播放器进入 error 态（多为缓存的直链已过期 / 被防盗链拒绝）：清缓存并自动重新解析一次。
 * 只对「本次起播复用了缓存」生效；清掉标记后不会再次自动重试，避免死循环。
 */
function onPlayerError() {
  if (!playingFromCache.value) return
  const attempt = lastAttempt.value
  clearResolveCache(playingCacheKey.value)
  playingFromCache.value = false
  playingCacheKey.value = ''
  if (!attempt) return
  ui.notify('缓存地址已失效，正在重新解析…', 'info')
  void playEpisode(attempt.ep, attempt.index)
}

function goBack() {
  // 返回按钮意味着真正离开详情页：先停播，返回父级后从 KeepAlive 中剔除，
  // 避免页面留在后台继续播放/占用内存。侧栏切走不走这里，仍保留缓存。
  writebackProgress()
  stopProgressTimer()
  resetPlayback()

  const back = route.meta.back
  const target = typeof back === 'string' && back.startsWith('/') ? back : '/vod'
  void router.push(target).then(() => {
    evictDetailView()
  })
}

function goList() {
  goBack()
}
</script>

<template>
  <PageScroll>
    <div class="page detail-page">
    <!-- 顶部操作条：返回（圆形图标按钮）+ 影视名称 + 站点名（纯文字，不用 tag）+ 刷新 -->
    <header class="page-header">
      <div class="heading">
        <el-button circle aria-label="返回上一页" @click="goBack">
          <template #icon><SolarIcon name="arrow-left" /></template>
        </el-button>
        <span v-if="detail?.name" class="heading-title ellipsis-1">{{ detail.name }}</span>
        <span v-if="detail?.name && siteName" class="heading-sep" aria-hidden="true">·</span>
        <span v-if="siteName" class="heading-site ellipsis-1">{{ siteName }}</span>
      </div>
      <el-button
        circle
        :disabled="loading"
        aria-label="重新加载详情"
        @click="loadDetail"
      >
        <template #icon><SolarIcon name="refresh" :size="14" /></template>
      </el-button>
    </header>

    <!-- 演示数据提示条 -->
    <div v-if="config.demoMode" class="demo-hint">
      <SolarIcon :size="12" name="info-circle" />
      <span>演示数据：当前为浏览器预览模式，详情与选集来自 mock 适配层；本地播放器需桌面端</span>
    </div>

    <!-- ① 加载骨架：左侧 16:9 播放器 + 右侧信息面板 -->
    <div v-if="loading" class="watch-skeleton" aria-label="详情加载中">
      <el-skeleton animated>
        <template #template>
          <div class="sk-layout">
            <el-skeleton-item variant="image" class="sk-player" />
            <div class="sk-info">
              <div class="sk-head">
                <el-skeleton-item variant="image" class="sk-poster" />
                <div class="sk-head-lines">
                  <el-skeleton-item variant="h1" class="sk-title" />
                  <el-skeleton-item variant="text" class="sk-text-60" />
                  <el-skeleton-item variant="text" class="sk-text-80" />
                </div>
              </div>
              <el-skeleton-item variant="text" class="sk-text-full" />
              <el-skeleton-item variant="text" class="sk-text-full" />
              <el-skeleton-item variant="text" class="sk-text-80" />
            </div>
          </div>
        </template>
      </el-skeleton>
    </div>

    <!-- ② 错误 -->
    <ErrorState
      v-else-if="error"
      :message="error"
      title="加载详情失败"
      retry-text="重试"
      @retry="loadDetail"
    />

    <!-- ③ 空 -->
    <EmptyState
      v-else-if="!detail"
      title="详情为空"
      desc="后端未返回该资源详情，可能已下架或换源"
      action-text="回列表"
      @action="goList"
    />

    <!-- ④ 正常：上排（播放器 + 信息面板），下排（整行选集） -->
    <template v-else>
      <section class="watch-layout">
        <div ref="playerCol" class="player-col">
          <PlayerPane
            ref="playerPane"
            :title="playingTitle"
            :series-name="detail.name"
            :episode-label="playing ? `第${playing.index + 1}集` : ''"
            :episodes="episodes"
            :active-episode="playing?.index ?? -1"
            :marker-key="`${siteKey}/${videoId}`"
            :cover="detail.cover"
            :has-media="Boolean(playingUrl)"
            :loading="resolving"
            :loading-text="paneLoadingText"
            :error-text="parseErrorText"
            :auto-save="autoSaveEffective"
            :save-info="playerSaveInfo"
            @toggle-auto-save="onToggleAutoSave"
            @retry="onPlayerRetry"
            @error="onPlayerError"
            @prev="playPrevEpisode"
            @next="playNextEpisode"
            @select-episode="playEpisodeAt"
          />
        </div>

        <aside class="info-panel panel" :style="infoPanelStyle">
          <DetailHero
            :detail="detail"
            :site-name="siteName"
            :category="categoryName"
            :download-loading="downloading"
            :download-all-loading="downloadingAll"
            :download-disabled="!hasEpisodes"
            :favorited="favorited"
            :favorite-loading="favoriteLoading"
            @play="playFirst"
            @download="onDownload"
            @download-all="onDownloadAll"
            @toggle-favorite="onToggleFavorite"
          />
        </aside>
      </section>

      <!-- 选集行：左列选集（6）+ 右列解析过程（4），窗口变窄也不换行 -->
      <section class="episodes" :class="{ 'has-parse': parseVisible }">
        <div class="episodes-main panel">
          <div v-if="!hasEpisodes" class="no-episodes muted">该资源暂无可播放线路</div>
          <template v-else>
            <div class="episodes-head">
              <span class="episodes-title">选集</span>
              <span class="episodes-count muted">共 {{ episodes.length }} 集</span>
              <span class="episodes-spacer" />
              <!-- 解析器选择（数据来自 config.parses；选择后 set_default_parse，并用于本页 resolve） -->
              <div v-if="parsers.length" class="parse-row">
                <el-select
                  v-model="selectedParse"
                  class="parse-select"
                  size="small"
                  clearable
                  placeholder="解析器（自动）"
                  @change="onParseChange"
                >
                  <el-option
                    v-for="parser in parsers"
                    :key="parser.name"
                    :label="parser.type === 4 ? `${parser.name}（超解析）` : `${parser.name}（type ${parser.type}）`"
                    :value="parser.name"
                  />
                </el-select>
              </div>
              <el-button
                size="small"
                :disabled="parseState === 'idle'"
                :title="parseVisible ? '隐藏解析过程面板' : '显示解析过程面板'"
                @click="onToggleParseLog"
              >
                <template #icon>
                  <SolarIcon :name="parseVisible ? 'close-circle' : 'info-circle'" :size="14" />
                </template>
                {{ parseVisible ? '隐藏解析' : '解析过程' }}
              </el-button>
            </div>

            <EpisodeLines :groups="lines" :active-line="activeLine" @select="onLineSelect" />
            <EpisodeGrid :items="episodes" :active-ep="activeEp" @play="onEpisodePlay" />
          </template>
        </div>

        <!-- 解析进度面板（内联在和选集并排的右列；事件订阅与计时在 useParseProgress 内管理） -->
        <ParseProgressPanel
          class="episodes-parse"
          inline
          :visible="parseVisible"
          :state="parseState"
          :title="parseTitle"
          :steps="parseSteps"
          :elapsed-ms="parseElapsedMs"
          :stage="parseStage"
          :error-message="parseError"
          @cancel="onParseCancel"
          @retry="onParseRetry"
          @close="closeParse"
        />
      </section>
    </template>

    <!-- 下载前选择线路（清晰度）/ 范围 -->
    <DownloadDialog
      v-model:open="downloadDialogOpen"
      :lines="lines"
      :active-line="activeLine"
      :active-index="downloadActiveIndex"
      :default-scope="downloadDialogDefaultScope"
      :loading="downloading || downloadingAll"
      @confirm="onDownloadConfirm"
    />
    </div>
  </PageScroll>
</template>

<style scoped>
/* 详情页撑满内容区可用高度：页头 / 提示条 / 播放行按内容高度，选集行吃掉剩余空间。
   高度 = 视口 - 标题栏（内容区滚动容器就是这么高），页面内容超出时仍正常滚动。 */
.detail-page {
  display: flex;
  flex-direction: column;
  min-height: calc(100vh - var(--titlebar-h));
}

.heading {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

/* 顶部标题：纯文字（影视名称在前，站点名在后），不再使用 tag */
.heading-title {
  min-width: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  letter-spacing: 0.2px;
}

.heading-sep {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-faint);
}

.heading-site {
  min-width: 0;
  font-size: 12.5px;
  color: var(--text-muted);
}

.demo-hint {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-3);
  font-size: 12px;
  color: var(--text-muted);
  background: var(--info-weak);
  border: 1px solid var(--border-lighter);
  border-radius: var(--radius-md);
}

.demo-hint .solar-icon {
  flex-shrink: 0;
  color: var(--info);
}

/* ---------------- 上排：播放器（6）+ 信息面板（4），任何宽度都不换行 ---------------- */

.watch-layout {
  display: grid;
  /* 固定 6:4：窗口变窄时两列一起收窄，不折成上下两行；
     播放器保持 16:9（宽度决定高度），右列信息面板高度跟随播放器，
     因此窗口变窄时被压缩的是右侧影视介绍的可视高度/宽度 */
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  gap: var(--space-4);
  align-items: stretch;
  /* 播放行按内容高度，不参与剩余高度分配 */
  flex: 0 0 auto;
  min-width: 0;
}

.player-col {
  /* 不要 stretch：否则列高会被右侧 info-panel 的定高反向撑大，
     形成「info 高 → 行高 → 列高 → 观测到旧高度」的闭环，宽度变化时高度偶尔不更新 */
  align-self: start;
  width: 100%;
  min-width: 0;
}

.info-panel {
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding: var(--space-3);
  overflow: hidden;
  scrollbar-width: thin;
}

/* ---------------- 下排：选集行（选集 6 + 解析过程 4） ---------------- */

.episodes {
  display: grid;
  /* 默认只有选集一列；解析面板出现（parseVisible）时切成 6:4，始终不换行 */
  grid-template-columns: minmax(0, 1fr);
  gap: var(--space-4);
  /* 两列卡片都撑满这一行的剩余高度（选集 / 解析过程） */
  align-items: stretch;
  /* 吃掉页面剩余高度：窗口越高，栏目越高 */
  flex: 1 1 auto;
  min-width: 0;
}

.episodes.has-parse {
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
}

.episodes-main {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-3);
  min-width: 0;
  min-height: 0;
}

.episodes-parse {
  min-width: 0;
}

.episodes-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  row-gap: var(--space-2);
}

.episodes-spacer {
  flex: 1;
  min-width: 0;
}

.episodes-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.episodes-count {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.no-episodes {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  padding: var(--space-4);
  font-size: 13px;
}

.parse-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.parse-select {
  width: 172px;
}

/* ---------------- 骨架 ---------------- */

.watch-skeleton {
  padding: var(--space-3);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}

.sk-layout {
  display: grid;
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  gap: var(--space-4);
}

.sk-player {
  width: 100%;
  aspect-ratio: 16 / 9;
  border-radius: var(--radius-lg);
}

.sk-info {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  min-width: 0;
}

.sk-head {
  display: flex;
  gap: var(--space-3);
}

.sk-poster {
  flex: 0 0 104px;
  width: 104px;
  aspect-ratio: 2 / 3;
  border-radius: var(--radius-md);
}

.sk-head-lines {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
}

.sk-title {
  width: 64%;
  height: 24px;
}

.sk-text-full {
  width: 100%;
}

.sk-text-80 {
  width: 80%;
}

.sk-text-60 {
  width: 60%;
}
</style>
