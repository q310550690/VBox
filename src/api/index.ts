// ============================================================
// API 门面（frontend-design.md §8.3）
// 页面 / store 只 import 本文件：
//   - 订阅、点播、搜索、解析、抓包、直播、历史、下载：Tauri 桌面端真实调用 commands.rs / history / download 命令
//   - 浏览器预览（demoMode）：订阅/点播/搜索/直播走演示数据，历史/下载走 mock + localStorage
// 后端就绪后只需改本文件，页面零改动。
// ============================================================
import * as core from './core'
import { translateCatvodError } from '@/utils/catvodErrors'
import type {
  AddResult,
  Category,
  ConfigInspection,
  ConfigLine,
  DownloadAction,
  DownloadCleanupResult,
  DownloadFileInfo,
  DownloadSettings,
  DownloadStorageStatus,
  ImportResult,
  DailyStat,
  ErrorCodeStat,
  ErrorEvent,
  SourceHealth,
  SubscriptionPrefs,
  DownloadStartPayload,
  DownloadTask,
  DoubanHome,
  EpgProgram,
  EpgProgramRaw,
  FavoriteItem,
  HistoryItem,
  HistorySettings,
  FfmpegProvisionProgress,
  FfmpegStatus,
  LiveGroup,
  LiveSource,
  LoadedConfig,
  LogEntry,
  NetworkEvent,
  OnlineSubtitle,
  Parser,
  PlaybackCachePrefs,
  PlaybackCacheStatus,
  PlaybackPrefs,
  PlayResult,
  PlayerState,
  PluginOverview,
  SearchHit,
  SearchPageResult,
  SearchPrefs,
  SearchScope,
  SubtitleSource,
  Subscription,
  VideoDetail,
  VideoItem,
} from '@/types'
import {
  DEMO_CONFIG,
  DEMO_ENABLED,
  DEMO_SITES,
  DEMO_SUBSCRIPTIONS,
  mockAddSubscription,
  mockAddressGroup,
  mockGetDetail,
  mockListCategories,
  mockListVideos,
  mockRemoveSubscription,
  mockResolve,
} from '@/mock/sites'
import { mockEpg, mockLiveChannels, mockLiveGroups } from '@/mock/live'
import { mockDoubanHome } from '@/mock/douban'
import {
  clearHistory,
  loadHistory,
  loadHistorySettings,
  removeHistory,
  saveHistorySettings,
  upsertHistory,
} from '@/mock/history'
import {
  loadDownloads,
  mockDownloadControl,
  mockDownloadRemove,
  mockDownloadStart,
  saveDownloads,
} from '@/mock/downloads'
import {
  addFavorite,
  isFavorite,
  loadFavorites,
  removeFavorite,
} from '@/mock/favorites'

/** 是否运行在 Tauri 窗口内（浏览器预览为 false） */
export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/** 浏览器预览模式：订阅/点播/搜索能力使用演示数据（页面需展示标识） */
export const demoMode = !isTauri

/** 尚未由后端提供、当前只能 mock 的能力（页面据此显示「待接入 · 演示数据」） */
export const MOCK_CAPABILITIES = {
  search: false,
  live: false,
  history: false,
  downloads: false,
}

/** 统一把后端返回的字符串错误 / JS Error 转成可读文案 */
export function formatError(e: unknown): string {
  let raw: string
  if (typeof e === 'string') raw = e
  else if (e instanceof Error) raw = e.message
  else if (e && typeof e === 'object' && 'message' in e) {
    const msg = (e as { message?: unknown }).message
    raw = typeof msg === 'string' ? msg : String(e)
  } else raw = String(e)
  // 家族/源层抛出的稳定码 → 面向用户的标题/建议动作；无码时保留原文
  return translateCatvodError(raw) ?? raw
}

/* ---------------- 演示数据适配（只在本文件内使用） ---------------- */

/** 旧 mock 的地址组 → 新契约 ConfigInspection */
function demoInspection(url: string): ConfigInspection {
  const group = mockAddressGroup(url)
  return {
    kind: group.kind,
    lines: group.addresses.map((a): ConfigLine => ({ name: a.name, url: a.url })),
    cacheHit: false,
  }
}

/** 构造演示模式的 LoadedConfig（模拟后端单仓/多仓加载结果） */
function demoLoaded(url: string, sourceUrl?: string): LoadedConfig {
  const input = sourceUrl || DEMO_SUBSCRIPTIONS[0]?.url || url
  const inspection = demoInspection(input)
  const active = inspection.lines.find((line) => line.url === url) ?? inspection.lines[0]
  return {
    input,
    sourceUrl: input,
    requestUrl: input,
    activeLine: active?.url ?? url,
    kind: inspection.kind,
    lines: inspection.lines.map((line) => ({ ...line })),
    config: DEMO_CONFIG,
    format: 'plain',
    cacheHit: false,
  }
}

/** 演示搜索：按站点/全部站点过滤 mock 片单（不伪造真实跨站检索） */
function demoSearch(keyword: string, scope: SearchScope, site?: string): SearchHit[] {
  const kw = keyword.trim().toLowerCase()
  if (!kw) return []
  const targets =
    scope === 'all' ? DEMO_SITES : DEMO_SITES.filter((s) => s.key === site)
  const hits: SearchHit[] = []
  for (const target of targets) {
    for (const category of ['1', '2', '3', '4']) {
      for (let page = 1; page <= 3; page += 1) {
        for (const item of mockListVideos(target.key, category, page)) {
          if (item.name.toLowerCase().includes(kw)) {
            hits.push({ ...item, siteKey: target.key, siteName: target.name })
          }
        }
      }
    }
  }
  return hits.slice(0, 60)
}

/* ---------------- 订阅管理 ---------------- */

export async function getSubscriptions(): Promise<Subscription[]> {
  if (demoMode) return DEMO_SUBSCRIPTIONS.map((s) => ({ ...s }))
  return core.getSubscriptions()
}

export async function getEnabled(): Promise<string | null> {
  if (demoMode) return DEMO_ENABLED
  return core.getEnabled()
}

export async function getLoaded(): Promise<LoadedConfig | null> {
  if (demoMode) return demoLoaded(DEMO_ENABLED)
  return core.getLoaded()
}

export async function addSubscription(name: string, url: string): Promise<AddResult> {
  if (!demoMode) return core.addSubscription(name, url)
  const res = mockAddSubscription(name, url)
  const active = res.enabled ?? ''
  return {
    subscriptions: res.subscriptions,
    enabled: res.enabled,
    loaded: active ? demoLoaded(active) : null,
    config: res.config,
  }
}

export async function removeSubscription(url: string): Promise<Subscription[]> {
  if (demoMode) return mockRemoveSubscription(url)
  return core.removeSubscription(url)
}

export async function getAddresses(name: string, url: string): Promise<ConfigInspection> {
  if (!demoMode) return core.getAddresses(name, url)
  const inspection = demoInspection(url)
  // 单仓时用订阅名作为地址名（与后端 get_addresses 行为一致）
  if (inspection.kind === 'single') {
    inspection.lines = inspection.lines.map((line) => ({ ...line, name: name || line.name }))
  }
  return inspection
}

export async function setEnabled(
  url: string,
  forceRefresh = false,
  sourceUrl?: string,
  lineUrl?: string,
): Promise<LoadedConfig> {
  if (demoMode) return demoLoaded(url, sourceUrl)
  return core.setEnabled(url, forceRefresh, sourceUrl, lineUrl)
}

/** 恢复上次启用的线路（重启后保持多仓 sourceUrl/activeLine） */
export async function restoreEnabled(forceRefresh = false): Promise<LoadedConfig> {
  if (demoMode) return demoLoaded(DEMO_ENABLED)
  return core.restoreEnabled(forceRefresh)
}

export async function reloadEnabled(forceRefresh = false): Promise<LoadedConfig> {
  if (demoMode) return demoLoaded(DEMO_ENABLED)
  return core.reloadEnabled(forceRefresh)
}

/* ---------------- 点播内容 ---------------- */

export async function listCategories(site: string, force = false): Promise<Category[]> {
  if (demoMode) return mockListCategories(site)
  return core.listCategories(site, force)
}

/** 分类标签磁盘缓存（只读、不联网）：启动先渲染，再后台刷新；演示模式无缓存返回 null */
export async function listCategoriesCached(site: string): Promise<Category[] | null> {
  if (demoMode) return null
  return core.listCategoriesCached(site)
}

export async function listVideos(site: string, category: string, page: number): Promise<VideoItem[]> {
  if (demoMode) return mockListVideos(site, category, page)
  return core.listVideos(site, category, page)
}

export async function getDetail(site: string, id: string): Promise<VideoDetail> {
  if (demoMode) return mockGetDetail(site, id)
  return core.getDetail(site, id)
}

/* ---------------- 豆瓣首页（点播首页信息源） ---------------- */

/** 豆瓣首页数据：热门电影 / 热门剧集 / 热门综艺 / 热门动漫 / TOP250 等 + 10 条轮播。
 *  桌面端走 Rust `douban_home`（带 10 分钟缓存与图片代理）；浏览器预览用演示数据。 */
export async function doubanHome(force = false): Promise<DoubanHome> {
  if (demoMode) return mockDoubanHome(force)
  return core.doubanHome(force)
}

/** 豆瓣首页磁盘缓存（只读、不联网）：启动先渲染，再后台增量刷新；演示模式返回 null */
export async function doubanHomeCached(): Promise<DoubanHome | null> {
  if (demoMode) return null
  return core.doubanHomeCached()
}

/* ---------------- 搜索（Tauri 真实调用；demo 走 mock） ---------------- */

export async function searchVideos(
  keyword: string,
  opts: { site?: string; scope?: SearchScope } = {},
): Promise<SearchHit[]> {
  const kw = keyword.trim()
  if (!kw) return []
  const scope = opts.scope ?? 'current'
  if (demoMode) return demoSearch(kw, scope, opts.site)
  return core.search({ site: opts.site, keyword: kw, scope })
}

/** 分页搜索（后端 `search_page`）：首次检索缓存 5 分钟，翻页只切片 */
export async function searchPageVideos(
  keyword: string,
  opts: { site?: string; scope?: SearchScope; page?: number; pageSize?: number } = {},
): Promise<SearchPageResult> {
  const kw = keyword.trim()
  const page = Math.max(1, opts.page ?? 1)
  const pageSize = Math.min(200, Math.max(1, opts.pageSize ?? 60))
  if (!kw) return { items: [], total: 0, page, pageSize, hasMore: false }
  if (demoMode) {
    const all = demoSearch(kw, opts.scope ?? 'current', opts.site)
    const start = (page - 1) * pageSize
    return {
      items: all.slice(start, start + pageSize),
      total: all.length,
      page,
      pageSize,
      hasMore: start + pageSize < all.length,
    }
  }
  return core.searchPage({
    site: opts.site,
    keyword: kw,
    scope: opts.scope,
    page,
    pageSize,
  })
}

/** 浏览器预览的搜索设置占位（真实值由桌面端 search.json 提供） */
const DEMO_SEARCH_PREFS_KEY = 'vbox.search.prefs'
const DEMO_DEFAULT_SEARCH_PREFS: SearchPrefs = { threads: 8 }

/** 读取搜索设置（跨源搜索线程数） */
export async function searchPrefs(): Promise<SearchPrefs> {
  if (!demoMode) return core.searchPrefs()
  try {
    const raw = localStorage.getItem(DEMO_SEARCH_PREFS_KEY)
    if (!raw) return { ...DEMO_DEFAULT_SEARCH_PREFS }
    const data = JSON.parse(raw) as Partial<SearchPrefs>
    return { ...DEMO_DEFAULT_SEARCH_PREFS, ...data }
  } catch {
    return { ...DEMO_DEFAULT_SEARCH_PREFS }
  }
}

/** 保存搜索设置，返回生效后的值（线程数归一化到 1-32） */
export async function searchSetPrefs(prefs: SearchPrefs): Promise<SearchPrefs> {
  const threads = Math.min(32, Math.max(1, Math.round(prefs.threads)))
  const next: SearchPrefs = { threads }
  if (demoMode) {
    try {
      localStorage.setItem(DEMO_SEARCH_PREFS_KEY, JSON.stringify(next))
    } catch {
      /* 忽略 */
    }
    return next
  }
  return core.searchSetPrefs(next)
}

/* ---------------- 解析 / 抓包 ---------------- */

export interface ResolveRequest {
  site: string
  /** 当前播放线路组名（EpisodeGroup.group） */
  flag?: string
  url?: string
  id?: string
  parseName?: string
  /** 本次解析的请求 id：后端按此 id 推送 parse-progress，并用于 cancelParse */
  requestId?: string
}

export interface SniffRequest {
  url: string
  headers?: Record<string, string>
  click?: string
  timeoutMs?: number
  /** 与 resolve 同一次解析的请求 id（进度事件续传） */
  requestId?: string
}

/** 播放解析：直链 / 解析接口 / 超解析；sniff=true 时还需调用 sniff() */
export async function resolveSource(req: ResolveRequest): Promise<PlayResult> {
  if (demoMode) {
    const res = mockResolve(req.url || req.id || '')
    return {
      ...res,
      sniff: res.sniff ?? false,
      parseName: res.parseName ?? null,
      trace: res.trace ?? ['演示数据：直链判定'],
    }
  }
  return core.resolve({
    site: req.site,
    flag: req.flag,
    url: req.url,
    id: req.id,
    parseName: req.parseName,
    requestId: req.requestId,
  })
}

/** WebView 抓包（浏览器演示模式不可用） */
export async function sniff(req: SniffRequest): Promise<PlayResult> {
  if (demoMode) {
    throw new Error('浏览器演示模式无法抓包（需桌面端 WebView）')
  }
  return core.sniff({
    url: req.url,
    headers: req.headers,
    click: req.click,
    timeoutMs: req.timeoutMs,
    requestId: req.requestId,
  })
}

/**
 * 取消本次解析/抓包（尽力而为）。
 * 后端可能已经完成并返回结果，页面仍应以 resolve/sniff 的最终返回为准。
 */
export async function cancelParse(requestId: string): Promise<boolean> {
  if (demoMode || !requestId) return false
  return core.cancelParse(requestId)
}

/* ---------------- 解析器 ---------------- */

export async function listParses(): Promise<Parser[]> {
  if (demoMode) return (DEMO_CONFIG.parses ?? []).map((p) => ({ ...p }))
  return core.listParses()
}

export async function setDefaultParse(name: string): Promise<void> {
  if (demoMode) return
  return core.setDefaultParse(name)
}

/* ---------------- 抓包日志 ---------------- */

export async function networkLog(limit?: number): Promise<NetworkEvent[]> {
  if (demoMode) return []
  return core.networkLog(limit)
}

export async function networkClear(): Promise<void> {
  if (demoMode) return
  return core.networkClear()
}

/* ---------------- 运行日志 ---------------- */

/** 最近运行日志（浏览器预览返回空数组，不伪造日志） */
export async function logsRecent(limit?: number): Promise<LogEntry[]> {
  if (demoMode) return []
  return core.logsRecent(limit)
}

export async function logsClear(): Promise<void> {
  if (demoMode) return
  return core.logsClear()
}

/** 导出日志；用户取消返回 null */
export async function logsExport(limit?: number): Promise<string | null> {
  if (demoMode) return null
  return core.logsExport(limit)
}

/** 打开独立的运行日志窗口（桌面端；演示模式忽略） */
export async function openLogWindow(): Promise<void> {
  if (demoMode) return
  return core.openLogWindow()
}

/* ---------------- 直播（Tauri 真实调用；demo 仍用 mock） ---------------- */

/** 配置里的直播源列表 */
export async function liveSources(): Promise<LiveSource[]> {
  if (demoMode) return (DEMO_CONFIG.lives ?? []).map((live) => ({ ...live }))
  return core.liveSources()
}

/** 加载某个 lives 组的频道；demo 用 mock 分组 + mock 频道拼装（补上演示多源地址以便体验换源） */
export async function liveGroups(index: number): Promise<LiveGroup[]> {
  if (!demoMode) return core.liveGroups(index)
  return mockLiveGroups().map((group) => ({
    ...group,
    channels: mockLiveChannels(group.key).map((channel, i) => ({
      ...channel,
      header: {},
      urls: [
        { url: `http://demo.local/live/${group.key}/${i + 1}.m3u8`, name: '演示源 A' },
        { url: `http://demo.local/live/${group.key}/${i + 1}-alt.m3u8`, name: '演示源 B' },
      ],
    })),
  }))
}

/** 解析 XMLTV 时间串（`20261028120000 +0800`）→ Unix 毫秒；失败返回 null */
function parseXmltvTime(value: string): number | null {
  const text = value.trim()
  if (!text) return null
  const m = /^(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})?\s*([+-]\d{4})?$/.exec(text)
  if (!m) return null
  const [, year, month, day, hour, minute, second, zone] = m
  const tz = zone ? `${zone.slice(0, 3)}:${zone.slice(3)}` : ''
  const ts = Date.parse(
    `${year}-${month}-${day}T${hour}:${minute}:${second ?? '00'}${tz}`,
  )
  return Number.isFinite(ts) ? ts : null
}

/** EPG：后端返回 XMLTV 原始串，这里转成 UI 用的毫秒时间 */
export async function liveEpg(params: {
  epgUrl: string
  channel: string
  date: string
}): Promise<EpgProgram[]> {
  if (demoMode) return mockEpg(params.channel)
  const raw: EpgProgramRaw[] = await core.liveEpg(params)
  return raw.map((program) => ({
    channel: program.channel,
    title: program.title,
    start: parseXmltvTime(program.start),
    end: parseXmltvTime(program.end),
  }))
}

/* ---------------- 播放器（libmpv 原生子窗口；demo 下全部安全降级） ---------------- */

/**
 * 浏览器预览用的「空闲」播放器状态。
 * 演示模式没有原生窗口，所有 player_* 命令都返回该默认值 / no-op，绝不抛错。
 */
export const PLAYER_IDLE_STATE: Readonly<PlayerState> = Object.freeze({
  status: 'idle',
  url: '',
  positionSec: 0,
  durationSec: 0,
  paused: false,
  buffering: false,
  volume: 100,
  muted: false,
  rate: 1,
  width: 0,
  height: 0,
  cacheSec: 0,
  bufferSpeedBps: 0,
  videoReady: false,
  containerFps: 0,
  estimatedFps: 0,
  hwdecCurrent: '',
  displayFps: 0,
  frameDrops: 0,
  decoderFrameDrops: 0,
  audioTracks: [],
  subtitleTracks: [],
  activeAudioId: 'no',
  activeSubtitleId: 'no',
  error: '',
})

/** 返回一份可变副本（避免页面误改冻结对象） */
export function playerIdleState(): PlayerState {
  return { ...PLAYER_IDLE_STATE }
}

/** 打开直链并起播（headers 用于防盗链；后端未就绪时抛错，由调用方降级为 error 态） */
export async function playerOpen(
  url: string,
  headers?: Record<string, string>,
  startSec?: number,
): Promise<void> {
  if (demoMode) return
  await core.playerOpen({ url, headers, startSec })
}

export async function playerClose(): Promise<void> {
  if (demoMode) return
  await core.playerClose()
}

export async function playerPlay(): Promise<void> {
  if (demoMode) return
  await core.playerPlay()
}

export async function playerPause(): Promise<void> {
  if (demoMode) return
  await core.playerPause()
}

/** 切换播放/暂停；demo 返回 false（未暂停） */
export async function playerTogglePause(): Promise<boolean> {
  if (demoMode) return false
  return core.playerTogglePause()
}

export async function playerSeek(sec: number): Promise<void> {
  if (demoMode) return
  await core.playerSeek(sec)
}

/** 音量 0-100 */
export async function playerSetVolume(volume: number): Promise<void> {
  if (demoMode) return
  await core.playerSetVolume(volume)
}

export async function playerSetMute(muted: boolean): Promise<void> {
  if (demoMode) return
  await core.playerSetMute(muted)
}

export async function playerSetRate(rate: number): Promise<void> {
  if (demoMode) return
  await core.playerSetRate(rate)
}

/** 切换音轨（mpv track id 字符串，'no' 关闭）；demo 无轨道，no-op */
export async function playerSetAudioTrack(id: string): Promise<void> {
  if (demoMode) return
  await core.playerSetAudioTrack(id)
}

/** 切换字幕轨（mpv track id 字符串，'no' 关闭）；demo 无轨道，no-op */
export async function playerSetSubtitleTrack(id: string): Promise<void> {
  if (demoMode) return
  await core.playerSetSubtitleTrack(id)
}

/** 加载外挂字幕（本地文件路径，后端校验存在并自动选中）；demo 无播放器，no-op */
export async function playerAddSubtitle(path: string): Promise<void> {
  if (demoMode) return
  await core.playerAddSubtitle(path)
}

/** 原生「选择字幕文件」对话框；取消 / 演示模式返回 null */
export async function playerPickSubtitle(): Promise<string | null> {
  if (demoMode) return null
  return core.playerPickSubtitle()
}

/* ---------------- 外部工具 FFmpeg（桌面端；浏览器预览仅占位） ---------------- */

const DEMO_FFMPEG_STATUS: FfmpegStatus = {
  path: '',
  effective: 'ffmpeg',
  source: 'path',
  available: false,
  version: null,
  error: '浏览器预览无法检测 FFmpeg（需桌面端）',
  bundled: null,
  managed: null,
  provisioning: false,
  canProvision: false,
}

/** 当前 FFmpeg 可用性（路径 / 版本 / 错误原因） */
export async function ffmpegStatus(): Promise<FfmpegStatus> {
  if (demoMode) return { ...DEMO_FFMPEG_STATUS }
  return core.ffmpegStatus()
}

/** 保存 FFmpeg 路径（null = 自动：已下载集成 → 随包集成 → VBOX_FFMPEG → PATH） */
export async function ffmpegSetPath(path: string | null): Promise<FfmpegStatus> {
  if (demoMode) throw new Error('浏览器预览无法修改 FFmpeg 路径（需桌面端）')
  return core.ffmpegSetPath(path)
}

/** 一键下载并集成 FFmpeg 到本机（约 105 MB，内置 SHA-256 校验）；返回最新状态 */
export async function ffmpegProvision(): Promise<FfmpegStatus> {
  if (demoMode) throw new Error('浏览器预览无法下载 FFmpeg（需桌面端）')
  return core.ffmpegProvision()
}

/** 订阅 FFmpeg 集成进度；返回取消订阅函数（演示模式为空实现） */
export async function onFfmpegProvision(
  callback: (progress: FfmpegProvisionProgress) => void,
): Promise<() => void> {
  if (demoMode) return () => {}
  const { listen } = await import('@tauri-apps/api/event')
  return listen<FfmpegProvisionProgress>('ffmpeg-provision', (event) => callback(event.payload))
}

/** 原生选择 ffmpeg 可执行文件；取消 / 演示模式返回 null */
export async function ffmpegPick(): Promise<string | null> {
  if (demoMode) return null
  return core.ffmpegPick()
}

/** 打开 ffmpeg 官方下载页；演示模式忽略 */
export async function ffmpegOpenDownload(): Promise<void> {
  if (demoMode) return
  await core.ffmpegOpenDownload()
}

/**
 * 保存当前 HLS 播放会话到下载目录（边看边存）：
 * 已缓存分片复用，未看部分补下后合并为 .ts / .mp4，并登记为一条已完成下载记录。
 */
export async function playerSaveCurrent(info: DownloadFileInfo): Promise<string> {
  if (demoMode) throw new Error('浏览器预览无法保存播放缓存（需桌面端）')
  return core.playerSaveCurrent(info)
}

/** 暂停当前「保存本集」任务；返回是否成功进入暂停态 */
export async function playerSavePause(): Promise<boolean> {
  if (demoMode) return false
  return core.playerSavePause()
}

/** 继续当前「保存本集」任务；返回是否成功恢复 */
export async function playerSaveResume(): Promise<boolean> {
  if (demoMode) return false
  return core.playerSaveResume()
}

/**
 * 「播放时自动保存」改用 HLS 代理：只登记导出目标，不再另起下载任务（单份流量）。
 * 返回 false 表示当前没有 HLS 代理会话（mp4 直链 / 代理降级），调用方应回退下载队列。
 */
export async function playerArmAutoSave(payload: {
  siteKey: string
  siteName?: string
  line?: string
  vodId?: string
  epIndex?: number
  name: string
  epTitle?: string
  cover?: string
}): Promise<boolean> {
  if (demoMode) return false
  return core.playerArmAutoSave(payload)
}

/* ---------------- 在线字幕（桌面端 Rust；浏览器预览不可用） ---------------- */

/** 可用在线字幕源；demo 无网络能力，返回空数组 */
export async function subtitleSources(): Promise<SubtitleSource[]> {
  if (demoMode) return []
  return core.subtitleSources()
}

/** 在线字幕搜索（按关键词，page 从 1 开始） */
export async function subtitleSearch(
  source: string,
  query: string,
  page = 1,
): Promise<OnlineSubtitle[]> {
  if (demoMode) return []
  return core.subtitleSearch(source, query, page)
}

/** 下载在线字幕并返回本地文件路径（可直接交给 playerAddSubtitle） */
export async function subtitleDownload(
  source: string,
  id: string,
  name?: string,
): Promise<string> {
  if (demoMode) throw new Error('浏览器预览无法下载在线字幕（需桌面端）')
  return core.subtitleDownload(source, id, name)
}

/** 浏览器预览的播放设置占位（真实值由桌面端 player.json 提供） */
const DEMO_PREFS_KEY = 'vbox.playback.prefs'
const DEMO_DEFAULT_PREFS: PlaybackPrefs = { hwdec: 'auto-safe', volume: 100, rate: 1, enhance: 'off' }

/** 读取播放设置（硬解 / 默认倍速；volume 为上次退出时记住的音量，仅供内部回传） */
export async function playerPrefs(): Promise<PlaybackPrefs> {
  if (!demoMode) return core.playerPrefs()
  try {
    const raw = localStorage.getItem(DEMO_PREFS_KEY)
    if (!raw) return { ...DEMO_DEFAULT_PREFS }
    const data = JSON.parse(raw) as Partial<PlaybackPrefs>
    return { ...DEMO_DEFAULT_PREFS, ...data }
  } catch {
    return { ...DEMO_DEFAULT_PREFS }
  }
}

/** 保存播放设置，返回生效后的值 */
export async function playerSetPrefs(prefs: PlaybackPrefs): Promise<PlaybackPrefs> {
  if (!demoMode) return core.playerSetPrefs(prefs)
  try {
    localStorage.setItem(DEMO_PREFS_KEY, JSON.stringify(prefs))
  } catch {
    /* 忽略 */
  }
  return { ...prefs }
}

/** 画质增强模式（播放器页面「画质」开关）：off / lite / standard / strong，返回实际生效值 */
export async function playerSetEnhance(mode: string): Promise<string> {
  const normalized = ['off', 'lite', 'standard', 'strong'].includes(mode) ? mode : 'off'
  if (!demoMode) return core.playerSetEnhance(normalized)
  // 浏览器预览：只保存选择，没有任何真实渲染效果
  try {
    const prefs = await playerPrefs()
    localStorage.setItem(DEMO_PREFS_KEY, JSON.stringify({ ...prefs, enhance: normalized }))
  } catch {
    /* 忽略 */
  }
  return normalized
}

/* ---------------- 插件（GLSL 着色器 / lavfi 滤镜） ---------------- */

const DEMO_PLUGIN_OVERVIEW: PluginOverview = {
  root: '',
  disabled: false,
  lastError: '',
  plugins: [],
}

/** 插件列表（浏览器预览为空列表） */
export function pluginOverview(): Promise<PluginOverview> {
  return demoMode ? Promise.resolve({ ...DEMO_PLUGIN_OVERVIEW }) : core.pluginOverview()
}

export function pluginRescan(): Promise<PluginOverview> {
  return demoMode ? Promise.resolve({ ...DEMO_PLUGIN_OVERVIEW }) : core.pluginRescan()
}

export function pluginSetEnabled(id: string, enabled: boolean): Promise<PluginOverview> {
  return demoMode ? Promise.resolve({ ...DEMO_PLUGIN_OVERVIEW }) : core.pluginSetEnabled(id, enabled)
}

export function pluginSetOrder(order: string[]): Promise<PluginOverview> {
  return demoMode ? Promise.resolve({ ...DEMO_PLUGIN_OVERVIEW }) : core.pluginSetOrder(order)
}

export function pluginSetPreset(pluginId: string, preset: string): Promise<PluginOverview> {
  return demoMode
    ? Promise.resolve({ ...DEMO_PLUGIN_OVERVIEW })
    : core.pluginSetPreset(pluginId, preset)
}

export function pluginOpenDir(): Promise<void> {
  return demoMode ? Promise.resolve() : core.pluginOpenDir()
}

export function pluginOpenUrl(url: string): Promise<void> {
  return demoMode ? Promise.resolve() : core.pluginOpenUrl(url)
}

/* ---------------- 播放缓存保留策略 ---------------- */

/** 浏览器预览的播放缓存占位（真实缓存只在桌面端存在） */
const DEMO_CACHE_PREFS_KEY = 'vbox.playback.cache.prefs'
const DEMO_DEFAULT_CACHE_PREFS: PlaybackCachePrefs = { maxBytes: 0, retentionDays: 0 }

/** 读取播放缓存保留策略（大小上限 / 保留天数，0 = 不限制） */
export async function playbackCachePrefs(): Promise<PlaybackCachePrefs> {
  if (!demoMode) return core.playbackCachePrefs()
  try {
    const raw = localStorage.getItem(DEMO_CACHE_PREFS_KEY)
    if (!raw) return { ...DEMO_DEFAULT_CACHE_PREFS }
    return { ...DEMO_DEFAULT_CACHE_PREFS, ...(JSON.parse(raw) as Partial<PlaybackCachePrefs>) }
  } catch {
    return { ...DEMO_DEFAULT_CACHE_PREFS }
  }
}

/** 保存播放缓存保留策略，返回生效值 */
export async function playbackCacheSetPrefs(
  prefs: PlaybackCachePrefs,
): Promise<PlaybackCachePrefs> {
  if (!demoMode) return core.playbackCacheSetPrefs(prefs)
  try {
    localStorage.setItem(DEMO_CACHE_PREFS_KEY, JSON.stringify(prefs))
  } catch {
    /* 忽略 */
  }
  return { ...prefs }
}

/** 播放缓存当前占用 + 生效策略（浏览器预览无本地缓存） */
export async function playbackCacheStatus(): Promise<PlaybackCacheStatus> {
  if (!demoMode) return core.playbackCacheStatus()
  const prefs = await playbackCachePrefs()
  return {
    dir: '（浏览器预览无本地播放缓存）',
    bytes: 0,
    files: 0,
    maxBytes: prefs.maxBytes,
    retentionDays: prefs.retentionDays,
  }
}

/** 清空播放缓存，返回清理后的状态 */
export async function playbackCacheClear(): Promise<PlaybackCacheStatus> {
  if (!demoMode) return core.playbackCacheClear()
  return playbackCacheStatus()
}

/** 上报视频区域矩形（物理像素，相对窗口客户区） */
export async function playerSetRect(rect: {
  x: number
  y: number
  width: number
  height: number
}): Promise<void> {
  if (demoMode) return
  await core.playerSetRect(rect)
}

export async function playerSetVisible(visible: boolean): Promise<void> {
  if (demoMode) return
  await core.playerSetVisible(visible)
}

/** 读取播放器状态；demo / 后端未就绪时返回空闲状态 */
export async function playerState(): Promise<PlayerState> {
  if (demoMode) return playerIdleState()
  try {
    return await core.playerState()
  } catch {
    return playerIdleState()
  }
}

/* ---------------- 收藏（桌面端 SQLite；浏览器预览 mock + localStorage） ---------------- */

export async function favoriteList(): Promise<FavoriteItem[]> {
  if (demoMode) return loadFavorites()
  return core.favoriteList()
}

export async function favoriteAdd(item: FavoriteItem): Promise<FavoriteItem[]> {
  if (demoMode) return addFavorite(item)
  return core.favoriteAdd(item)
}

export async function favoriteRemove(siteKey: string, vodId: string): Promise<FavoriteItem[]> {
  if (demoMode) return removeFavorite(siteKey, vodId)
  return core.favoriteRemove(siteKey, vodId)
}

export async function favoriteCheck(siteKey: string, vodId: string): Promise<boolean> {
  if (demoMode) return isFavorite(siteKey, vodId)
  return core.favoriteCheck(siteKey, vodId)
}

/* ---------------- 历史记录（桌面端 SQLite；浏览器预览 mock + localStorage） ---------------- */

export async function historyList(): Promise<HistoryItem[]> {
  if (demoMode) return loadHistory()
  return core.historyList()
}

export async function historyUpsert(item: HistoryItem): Promise<HistoryItem[]> {
  if (demoMode) return upsertHistory(item)
  return core.historyUpsert(item)
}

export async function historyRemove(siteKey: string, id: string): Promise<HistoryItem[]> {
  if (demoMode) return removeHistory(siteKey, id)
  return core.historyRemove(siteKey, id)
}

export async function historyClear(): Promise<HistoryItem[]> {
  if (demoMode) return clearHistory()
  return core.historyClear()
}

/** 保留策略：retentionDays=0 永久，maxItems=0 不限 */
export async function historySettings(): Promise<HistorySettings> {
  if (demoMode) return loadHistorySettings()
  return core.historySettings()
}

/** 保存保留策略（后端保存后立即裁剪；mock 同样即时裁剪） */
export async function historySetSettings(
  retentionDays: number,
  maxItems: number,
): Promise<HistorySettings> {
  if (demoMode) return saveHistorySettings({ retentionDays, maxItems })
  return core.historySetSettings(retentionDays, maxItems)
}

/* ---------------- 下载（桌面端 Rust 下载引擎；浏览器预览 mock + localStorage） ---------------- */

/** 浏览器预览的下载目录占位（真实目录由桌面端后端提供） */
const DEMO_DOWNLOAD_SETTINGS: DownloadSettings = {
  dir: '浏览器预览（仅演示）',
  defaultDir: '浏览器预览（仅演示）',
  autoSave: false,
  concurrency: 1,
  speedLimitKbps: 0,
  autoSaveLowPriority: true,
  autoSaveLimitKbps: 0,
}

export async function downloadList(): Promise<DownloadTask[]> {
  if (demoMode) return loadDownloads()
  return core.downloadList()
}

/** 新建下载任务（payload 中的 url 必须是已解析直链；demo 下生成模拟任务） */
export async function downloadStart(params: DownloadStartPayload): Promise<DownloadTask> {
  if (demoMode) return mockDownloadStart(params)
  return core.downloadStart(params)
}

/** 查询该集是否已有可用本地成品（completed 且文件仍存在）；演示模式没有本地文件返回 null */
export async function downloadLocalFile(info: DownloadFileInfo): Promise<DownloadTask | null> {
  if (demoMode) return null
  return core.downloadLocalFile(info)
}

/** 任务控制：pause / resume / cancel / retry，返回覆盖后的完整列表 */
export async function downloadControl(
  id: string,
  action: DownloadAction,
): Promise<DownloadTask[]> {
  if (demoMode) return mockDownloadControl(id, action)
  return core.downloadControl(id, action)
}

/** 删除任务记录；removeFile=false 时保留已下载文件 */
export async function downloadRemove(
  id: string,
  removeFile = false,
): Promise<DownloadTask[]> {
  if (demoMode) return mockDownloadRemove(id)
  return core.downloadRemove(id, removeFile)
}

/** 下载/缓存目录设置 */
export async function downloadSettings(): Promise<DownloadSettings> {
  if (demoMode) return { ...DEMO_DOWNLOAD_SETTINGS }
  return core.downloadSettings()
}

/** 修改下载目录；demo 返回原设置（不可修改） */
export async function downloadSetDir(dir: string): Promise<DownloadSettings> {
  if (demoMode) return { ...DEMO_DOWNLOAD_SETTINGS }
  return core.downloadSetDir(dir)
}

/** 播放时自动保存开关；demo 只更新内存返回值（浏览器预览不会真实下载） */
export async function downloadSetAutoSave(enabled: boolean): Promise<DownloadSettings> {
  if (demoMode) return { ...DEMO_DOWNLOAD_SETTINGS, autoSave: enabled }
  return core.downloadSetAutoSave(enabled)
}

/** 下载并发数 / 全局限速；demo 只更新内存返回值 */
export async function downloadSetNetwork(
  concurrency: number,
  speedLimitKbps: number,
): Promise<DownloadSettings> {
  if (demoMode) return { ...DEMO_DOWNLOAD_SETTINGS, concurrency, speedLimitKbps }
  return core.downloadSetNetwork(concurrency, speedLimitKbps)
}

/** 编辑订阅名称/链接；demo 直接返回原订阅列表 */
export async function updateSubscription(
  oldUrl: string,
  name: string,
  url: string,
): Promise<Subscription[]> {
  if (demoMode) return []
  return core.updateSubscription(oldUrl, name, url)
}

/** 导出订阅到 JSON；demo 不支持 */
export async function exportSubscriptions(): Promise<string | null> {
  if (demoMode) throw new Error('浏览器预览模式不支持导出订阅')
  return core.exportSubscriptions()
}

/** 从 JSON 导入订阅；demo 返回 null */
export async function importSubscriptions(): Promise<ImportResult | null> {
  if (demoMode) return null
  return core.importSubscriptions()
}

/** 订阅设置（定时刷新间隔） */
export async function subscriptionPrefs(): Promise<SubscriptionPrefs> {
  if (demoMode) return { refreshMinutes: 0 }
  return core.subscriptionPrefs()
}

/** 保存定时刷新间隔（分钟，0=关闭） */
export async function setSubscriptionRefresh(minutes: number): Promise<SubscriptionPrefs> {
  if (demoMode) return { refreshMinutes: minutes }
  return core.setSubscriptionRefresh(minutes)
}

/** 源健康度记录列表；demo 返回空 */
export async function sourceHealthList(): Promise<SourceHealth[]> {
  if (demoMode) return []
  return core.sourceHealthList()
}

/** 站点 api 支持画像（REA 产物）；demo 返回空数组 */
export async function siteApiSupport(): Promise<import('@/types').SiteApiSupport[]> {
  if (demoMode) return []
  return core.siteApiSupport()
}

/** 记录一次源请求结果（前端记录解析/搜索等失败） */
export async function sourceHealthRecord(
  kind: string,
  key: string,
  label: string,
  ok: boolean,
  error?: string,
): Promise<void> {
  if (demoMode) return
  try {
    await core.sourceHealthRecord(kind, key, label, ok, error)
  } catch {
    // 源健康记录失败不阻断业务
  }
}

/** 重置单个源健康记录（清零连续失败） */
export async function sourceHealthReset(kind: string, key: string): Promise<void> {
  if (demoMode) return
  await core.sourceHealthReset(kind, key)
}

/** 重置全部源健康记录 */
export async function sourceHealthResetAll(): Promise<void> {
  if (demoMode) return
  await core.sourceHealthResetAll()
}

/** 按 catvod 稳定码聚合的失败分布（demo 下为空） */
export async function sourceHealthCodeStats(): Promise<ErrorCodeStat[]> {
  if (demoMode) return []
  return core.sourceHealthCodeStats()
}

/** 最近失败事件（倒序，demo 下为空） */
export async function sourceHealthRecentEvents(limit = 20): Promise<ErrorEvent[]> {
  if (demoMode) return []
  return core.sourceHealthRecentEvents(limit)
}

/** 最近 N 天成败趋势（demo 下为空） */
export async function sourceHealthDaily(days = 7): Promise<DailyStat[]> {
  if (demoMode) return []
  return core.sourceHealthDaily(days)
}

/** 自动保存任务策略（低优先级 / 单独限速）；demo 只更新内存返回值 */
export async function downloadSetAutoSavePolicy(
  lowPriority: boolean,
  autoSaveLimitKbps: number,
): Promise<DownloadSettings> {
  if (demoMode) return { ...DEMO_DOWNLOAD_SETTINGS, autoSaveLowPriority: lowPriority, autoSaveLimitKbps }
  return core.downloadSetAutoSavePolicy(lowPriority, autoSaveLimitKbps)
}

/** 原生选择文件夹；demo 不支持，返回 null（调用方提示） */
export async function downloadPickDir(): Promise<string | null> {
  if (demoMode) return null
  return core.downloadPickDir()
}

/** 打开当前下载目录；demo 无本地目录，no-op */
export async function downloadOpenDir(): Promise<void> {
  if (demoMode) return
  await core.downloadOpenDir()
}

/** 打开某个任务文件所在位置；demo 无本地文件，抛错让页面提示 */
export async function downloadOpenFileLocation(id: string): Promise<void> {
  if (demoMode) throw new Error('浏览器预览模式不支持打开本地文件')
  await core.downloadOpenFileLocation(id)
}

/** 下载空间总览；demo 返回空统计 */
export async function downloadStorageStatus(): Promise<DownloadStorageStatus> {
  if (demoMode) {
    return {
      dir: DEMO_DOWNLOAD_SETTINGS.dir,
      diskTotalBytes: 0,
      diskFreeBytes: 0,
      usedBytes: 0,
      cacheBytes: 0,
      partBytes: 0,
      completedBytes: 0,
      partCount: 0,
      completedCount: 0,
    }
  }
  return core.downloadStorageStatus()
}

/** 清理 .part 残留；demo 无文件，返回 0 */
export async function downloadCleanupParts(): Promise<DownloadCleanupResult> {
  if (demoMode) return { removedFiles: 0, freedBytes: 0 }
  return core.downloadCleanupParts()
}

/** 清理已完成下载（文件 + 记录）；demo 返回 0 */
export async function downloadCleanupCompleted(): Promise<DownloadCleanupResult> {
  if (demoMode) return { removedFiles: 0, freedBytes: 0 }
  return core.downloadCleanupCompleted()
}

/** 仅演示模式使用：把 ticker 推进后的任务写回 localStorage */
export async function downloadSave(tasks: DownloadTask[]): Promise<DownloadTask[]> {
  return saveDownloads(tasks)
}

/* ---------------- Python 运行时 ---------------- */

export const pythonStatus = core.pythonStatus
export const setPythonPath = core.setPythonPath
export const refreshPythonStatus = core.refreshPythonStatus

/* ---------------- Node / drpy 运行时 ---------------- */

export const nodeStatus = core.nodeStatus
export const setNodePath = core.setNodePath
export const refreshNodeStatus = core.refreshNodeStatus

/* ---------------- 窗口外观 ---------------- */

/** 窗口是否已应用原生毛玻璃（Windows Acrylic）；浏览器预览恒为 false */
export async function windowBackdrop(): Promise<boolean> {
  if (!isTauri) return false
  try {
    return await core.windowBackdrop()
  } catch {
    return false
  }
}

/** 本地媒体 HTTP 地址（Range 流式）；失败时调用方回退到绝对路径 */
export function localMediaUrl(path: string): Promise<string> {
  return core.localMediaUrl(path)
}

/* ---------------- 网盘账号登录（W13） ---------------- */

// 只在 Tauri 下可用；浏览器演示模式请调用方先判 `demoMode` 再决定是否调用。
export * from './pan'

/* ---------------- 云盘聚合搜索（W14） ---------------- */

export * from './cloud'
export * from './drive'
