// 后端调用封装：通过 Tauri 的 invoke 调用 Rust 命令
// 命令名 / 参数一一对应 src-tauri/src/commands.rs 中的 #[tauri::command]
// 契约（2026-10 二期）：LoadedConfig 含 sourceUrl（合集入口）；set_enabled 支持 sourceUrl/lineUrl；
//                      restore_enabled 恢复上次线路；直播 = live_sources / live_groups / live_epg
import { invoke } from '@tauri-apps/api/core'
import type {
  AddResult,
  Category,
  ConfigInspection,
  DownloadAction,
  DownloadCleanupResult,
  DownloadFileInfo,
  DownloadSettings,
  DownloadStorageStatus,
  ImportResult,
  SourceHealth,
  SubscriptionPrefs,
  DownloadStartPayload,
  DownloadTask,
  DoubanHome,
  EpgProgramRaw,
  FavoriteItem,
  FfmpegStatus,
  HistoryItem,
  HistorySettings,
  LiveGroup,
  LiveSource,
  LoadedConfig,
  LogEntry,
  NetworkEvent,
  NodeStatus,
  OnlineSubtitle,
  Parser,
  PlaybackCachePrefs,
  PlaybackCacheStatus,
  PlaybackPrefs,
  PlayResult,
  PlayerState,
  PluginOverview,
  PythonStatus,
  JvmConvertSnapshot,
  JvmStatus,
  TelemetryStatus,
  SearchHit,
  SearchPageResult,
  SearchPrefs,
  SearchScope,
  SubtitleSource,
  Subscription,
  VideoDetail,
  VideoItem,
} from '@/types'

/* ---------------- 命令参数 ---------------- */

/** resolve：站点 + 线路 flag + 剧集 url/id + 指定解析器 */
export interface ResolveParams {
  site: string
  /** 播放线路组名（EpisodeGroup.group，即 vod_play_from 的一条） */
  flag?: string
  /** 剧集原始地址 */
  url?: string
  /** 剧集 id（url 缺省时后端会用 id 兜底） */
  id?: string
  /** 指定解析器名称；缺省用后端默认解析器 */
  parseName?: string
  /**
   * 本次解析的请求 id（前端生成）。
   * 后端按此 id 推送 `parse-progress` 事件，也用它匹配 `cancel_parse`。
   */
  requestId?: string
}

/** search：scope=current 时用 site，scope=all 时忽略 site */
export interface SearchParams {
  site?: string
  keyword: string
  scope?: SearchScope
}

/** sniff：WebView 抓包 */
export interface SniffParams {
  url: string
  headers?: Record<string, string>
  /** 嗅探自动点击脚本：`包含子串;JS` */
  click?: string
  timeoutMs?: number
  /** 与 resolve 同一次解析的请求 id（后端据此继续推送 parse-progress） */
  requestId?: string
}

/* ---------------- 订阅管理 ---------------- */

// 读取当前订阅列表
export function getSubscriptions() {
  return invoke<Subscription[]>('get_subscriptions')
}
// 读取当前启用的线路 url（未启用为 null）
export function getEnabled() {
  return invoke<string | null>('get_enabled')
}
// 读取当前已加载的配置（后端首次启动时可能为 null）
export function getLoaded() {
  return invoke<LoadedConfig | null>('get_loaded')
}
// 新增订阅（名称+链接均为必填）。首个订阅会自动启用并返回 loaded。
export function addSubscription(name: string, url: string) {
  return invoke<AddResult>('add_subscription', { name, url })
}
// 删除订阅，返回新列表
export function removeSubscription(url: string) {
  return invoke<Subscription[]>('remove_subscription', { url })
}
// 展开订阅：返回单仓/多仓（线路合集）与线路列表
export function getAddresses(name: string, url: string) {
  return invoke<ConfigInspection>('get_addresses', { name, url })
}
// 启用某个地址/线路。
// - 单配置：sourceUrl 省略，url 即配置地址
// - 线路合集：sourceUrl=合集入口，url=要激活的线路（lineUrl 可显式指定）
export function setEnabled(
  url: string,
  forceRefresh = false,
  sourceUrl?: string,
  lineUrl?: string,
) {
  return invoke<LoadedConfig>('set_enabled', { url, forceRefresh, sourceUrl, lineUrl })
}
// 恢复上次启用的线路（重启后保持多仓上下文）
export function restoreEnabled(forceRefresh = false) {
  return invoke<LoadedConfig>('restore_enabled', { forceRefresh })
}
// 重新加载当前启用线路（后端走 restore_enabled）
export function reloadEnabled(forceRefresh = false) {
  return invoke<LoadedConfig>('reload_enabled', { forceRefresh })
}

/* ---------------- 读取型命令（从当前 loaded 配置读） ---------------- */

// 列出某站点分类（force=true 跳过后端缓存，强制联网）
export function listCategories(site: string, force = false) {
  return invoke<Category[]>('list_categories', { site, force })
}
// 分类标签磁盘缓存（只读、不联网）：启动先用它渲染，再后台刷新
export function listCategoriesCached(site: string) {
  return invoke<Category[] | null>('list_categories_cached', { site })
}
// 列出某分类下的视频（分页）
export function listVideos(site: string, category: string, page: number) {
  return invoke<VideoItem[]>('list_videos', { site, category, page })
}
// 获取影视详情（含多线路选集）
export function getDetail(site: string, id: string) {
  return invoke<VideoDetail>('get_detail', { site, id })
}
// 豆瓣首页：热播电影 / 剧集 / 综艺 / 动漫等集合 + 10 条轮播（含简介）
// force=true 跳过后端 10 分钟缓存（刷新按钮）
export function doubanHome(force = false) {
  return invoke<DoubanHome>('douban_home', { force })
}
// 豆瓣首页磁盘缓存（只读、不联网）：启动先用它渲染，再后台增量刷新
export function doubanHomeCached() {
  return invoke<DoubanHome | null>('douban_home_cached')
}
// 搜索：scope=current 单站 / scope=all 全部站点并发（后端 60 条上限、8s 超时）
export function search(params: SearchParams) {
  return invoke<SearchHit[]>('search', {
    site: params.site,
    keyword: params.keyword,
    scope: params.scope ?? 'current',
  })
}
// 分页搜索：首次检索结果在 Rust 侧缓存 5 分钟，翻页只切片，不再重复打站点
export function searchPage(params: SearchParams & { page?: number; pageSize?: number }) {
  return invoke<SearchPageResult>('search_page', {
    site: params.site,
    keyword: params.keyword,
    scope: params.scope ?? 'current',
    page: params.page,
    pageSize: params.pageSize,
  })
}
// 搜索设置（跨源搜索线程数）
export function searchPrefs() {
  return invoke<SearchPrefs>('search_prefs')
}
// 保存搜索设置，返回生效后的值
export function searchSetPrefs(prefs: SearchPrefs) {
  return invoke<SearchPrefs>('search_set_prefs', { prefs })
}
// 播放解析：站点 + flag + 剧集 url/id + 指定解析器 → PlayResult
export function resolve(params: ResolveParams) {
  return invoke<PlayResult>('resolve', {
    site: params.site,
    flag: params.flag,
    url: params.url,
    id: params.id,
    parseName: params.parseName,
    // 进度事件（parse-progress）与取消（cancel_parse）都按这个 id 关联
    requestId: params.requestId,
  })
}

/* ---------------- 解析器 ---------------- */

// 当前配置里的解析器列表
export function listParses() {
  return invoke<Parser[]>('list_parses')
}
// 设置后端默认解析器（resolve 未显式传 parseName 时使用）
export function setDefaultParse(name: string) {
  return invoke<void>('set_default_parse', { name })
}

/* ---------------- 抓包 / 调试 ---------------- */

// 媒体地址嗅探（桌面端隐藏 WebView 抓包）
export function sniff(params: SniffParams) {
  return invoke<PlayResult>('sniff', {
    url: params.url,
    headers: params.headers,
    click: params.click,
    timeoutMs: params.timeoutMs,
    requestId: params.requestId,
  })
}
// 取消本次解析/抓包（尽力而为：后端可能已拿到结果，返回值表示是否受理了取消）
export function cancelParse(requestId: string) {
  return invoke<boolean>('cancel_parse', { requestId })
}
// VBox 自身 HTTP 请求日志
export function networkLog(limit?: number) {
  return invoke<NetworkEvent[]>('network_log', { limit })
}
export function networkClear() {
  return invoke<void>('network_clear')
}

/* ---------------- 运行日志 ---------------- */

// 最近的运行日志（旧 → 新，最多 500 条）
export function logsRecent(limit?: number) {
  return invoke<LogEntry[]>('logs_recent', { limit })
}
export function logsClear() {
  return invoke<void>('logs_clear')
}
// 导出日志到文本文件；用户取消返回 null
export function logsExport(limit?: number) {
  return invoke<string | null>('logs_export', { limit })
}
// 打开独立的「运行日志」窗口（已打开则聚焦）
export function openLogWindow() {
  return invoke<void>('open_log_window')
}

/* ---------------- 直播 ---------------- */

// 当前配置里的直播源列表
export function liveSources() {
  return invoke<LiveSource[]>('live_sources')
}
// 加载某个 lives 组的频道（type0 文本订阅 / type3 js）
export function liveGroups(index: number) {
  return invoke<LiveGroup[]>('live_groups', { index })
}
// 拉取并解析某个频道的 EPG（XMLTV；URL 模板由后端渲染）
export function liveEpg(params: { epgUrl: string; channel: string; date: string }) {
  return invoke<EpgProgramRaw[]>('live_epg', {
    epgUrl: params.epgUrl,
    channel: params.channel,
    date: params.date,
  })
}

/* ---------------- 播放器（libmpv 原生子窗口） ---------------- */

/** player_open：直链 + 请求头 + 起播位置（秒） */
export interface PlayerOpenParams {
  url: string
  headers?: Record<string, string>
  startSec?: number
}

// 打开直链（后端会替换当前媒体）
export function playerOpen(params: PlayerOpenParams) {
  return invoke<void>('player_open', {
    url: params.url,
    headers: params.headers,
    startSec: params.startSec,
  })
}
// 关闭并释放播放器
export function playerClose() {
  return invoke<void>('player_close')
}
// 继续播放
export function playerPlay() {
  return invoke<void>('player_play')
}
// 暂停
export function playerPause() {
  return invoke<void>('player_pause')
}
// 切换播放/暂停，返回切换后是否 paused
export function playerTogglePause() {
  return invoke<boolean>('player_toggle_pause')
}
// 跳转到指定秒
export function playerSeek(sec: number) {
  return invoke<void>('player_seek', { sec })
}
// 音量 0-100
export function playerSetVolume(volume: number) {
  return invoke<void>('player_set_volume', { volume })
}
// 静音开关
export function playerSetMute(muted: boolean) {
  return invoke<void>('player_set_mute', { muted })
}
// 播放倍速
export function playerSetRate(rate: number) {
  return invoke<void>('player_set_rate', { rate })
}
// 画质增强（Anime4K 超分）：off / lite / standard / strong，返回实际生效值
export function playerSetEnhance(mode: string) {
  return invoke<string>('player_set_enhance', { mode })
}

/* ---------------- 插件（GLSL 着色器 / lavfi 滤镜） ---------------- */

/** 插件列表 + 持久化状态（不触发磁盘重扫） */
export function pluginOverview() {
  return invoke<PluginOverview>('plugin_overview')
}

/** 重新扫描插件目录（新增/删除/改 manifest 后调用） */
export function pluginRescan() {
  return invoke<PluginOverview>('plugin_rescan')
}

/** 启用/禁用插件；返回最新总览（顺序、错误状态一并刷新） */
export function pluginSetEnabled(id: string, enabled: boolean) {
  return invoke<PluginOverview>('plugin_set_enabled', { id, enabled })
}

/** 按给定顺序应用启用列表（不在列表里的启用插件排到末尾） */
export function pluginSetOrder(order: string[]) {
  return invoke<PluginOverview>('plugin_set_order', { order })
}

/** 选择多预设插件（多着色器包）的当前预设 */
export function pluginSetPreset(pluginId: string, preset: string) {
  return invoke<PluginOverview>('plugin_set_preset', { id: pluginId, preset })
}

/** 在系统文件管理器里打开插件目录 */
export function pluginOpenDir() {
  return invoke<void>('plugin_open_dir')
}
// 在系统浏览器打开插件主页（后端只允许 http/https）
export function pluginOpenUrl(url: string) {
  return invoke<void>('plugin_open_url', { url })
}
// 上报视频区域矩形（物理像素，相对窗口客户区）
export function playerSetRect(rect: { x: number; y: number; width: number; height: number }) {
  return invoke<void>('player_set_rect', {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
  })
}
// 显示/隐藏原生视频窗口
export function playerSetVisible(visible: boolean) {
  return invoke<void>('player_set_visible', { visible })
}
// 读取当前播放器状态
export function playerState() {
  return invoke<PlayerState>('player_state')
}
// 切换音轨（id 为 mpv track id 字符串，'no' 表示关闭）
export function playerSetAudioTrack(id: string) {
  return invoke<void>('player_set_audio_track', { id })
}
// 切换字幕轨（id 为 mpv track id 字符串，'no' 表示关闭）
export function playerSetSubtitleTrack(id: string) {
  return invoke<void>('player_set_subtitle_track', { id })
}
// 加载外挂字幕（本地文件路径；后端会校验文件存在并自动选中新轨）
export function playerAddSubtitle(path: string) {
  return invoke<void>('player_add_subtitle', { path })
}
// 原生「选择字幕文件」对话框；取消返回 null
export function playerPickSubtitle() {
  return invoke<string | null>('player_pick_subtitle')
}
// 保存当前 HLS 代理会话为本地文件（边看边存），返回保存路径
// info 与后端 SavedFileInfo 对齐：站点 / 线路 / 影片 / 集，后端据此计算四级目录
export function playerSaveCurrent(info: DownloadFileInfo) {
  return invoke<string>('player_save_current', { info })
}
// 暂停 / 继续当前「保存本集」任务（返回状态是否切换成功）
export function playerSavePause() {
  return invoke<boolean>('player_save_pause')
}
export function playerSaveResume() {
  return invoke<boolean>('player_save_resume')
}
// FFmpeg 状态（路径解析 + -version 探测）
export function ffmpegStatus() {
  return invoke<FfmpegStatus>('ffmpeg_status')
}
// 保存 FFmpeg 路径（null/空串 = 自动），返回最新状态
export function ffmpegSetPath(path: string | null) {
  return invoke<FfmpegStatus>('ffmpeg_set_path', { path })
}
// 下载并集成 FFmpeg（后台执行；进度见 `ffmpeg-provision` 事件），返回最新状态
export function ffmpegProvision() {
  return invoke<FfmpegStatus>('ffmpeg_provision')
}
// 原生「选择 ffmpeg 可执行文件」对话框；取消返回 null
export function ffmpegPick() {
  return invoke<string | null>('ffmpeg_pick')
}
// 打开 ffmpeg 官方下载页
export function ffmpegOpenDownload() {
  return invoke<void>('ffmpeg_open_download')
}
// 「播放时自动保存」：把本集登记为 HLS 代理导出目标；返回 false 表示当前无 HLS 会话（应回退下载队列）
export function playerArmAutoSave(info: {
  siteKey: string
  siteName?: string
  line?: string
  vodId?: string
  epIndex?: number
  name: string
  epTitle?: string
  cover?: string
}) {
  return invoke<boolean>('player_arm_autosave', { info })
}
// 可用在线字幕源
export function subtitleSources() {
  return invoke<SubtitleSource[]>('subtitle_sources')
}
// 在线字幕搜索（page 从 1 开始）
export function subtitleSearch(source: string, query: string, page = 1) {
  return invoke<OnlineSubtitle[]>('subtitle_search', { source, query, page })
}
// 下载在线字幕到本地缓存，返回本地文件路径
export function subtitleDownload(source: string, id: string, name?: string) {
  return invoke<string>('subtitle_download', { source, id, name })
}
// 读取播放设置（硬解 / 默认倍速；volume 是上次退出时记住的音量）
export function playerPrefs() {
  return invoke<PlaybackPrefs>('player_prefs')
}
// 保存播放设置，返回生效后的值
export function playerSetPrefs(prefs: PlaybackPrefs) {
  return invoke<PlaybackPrefs>('player_set_prefs', { prefs })
}
// 播放缓存保留策略（大小上限 / 保留天数，0 = 不限制）
export function playbackCachePrefs() {
  return invoke<PlaybackCachePrefs>('playback_cache_prefs')
}
// 保存播放缓存策略（后端保存后立即按新策略清理一次），返回生效值
export function playbackCacheSetPrefs(prefs: PlaybackCachePrefs) {
  return invoke<PlaybackCachePrefs>('playback_cache_set_prefs', { prefs })
}
// 播放缓存当前占用 + 生效策略
export function playbackCacheStatus() {
  return invoke<PlaybackCacheStatus>('playback_cache_status')
}
// 清空播放缓存，返回清理后的状态
export function playbackCacheClear() {
  return invoke<PlaybackCacheStatus>('playback_cache_clear')
}

/* ---------------- 收藏（SQLite） ---------------- */

// 读取全部收藏（createdAt 倒序）
export function favoriteList() {
  return invoke<FavoriteItem[]>('favorite_list')
}
// 新增/更新一条收藏，返回更新后的完整列表
export function favoriteAdd(item: FavoriteItem) {
  return invoke<FavoriteItem[]>('favorite_add', { item })
}
// 取消收藏（按站点 + 影片），返回更新后的完整列表
export function favoriteRemove(siteKey: string, vodId: string) {
  return invoke<FavoriteItem[]>('favorite_remove', { siteKey, vodId })
}
// 单条收藏状态
export function favoriteCheck(siteKey: string, vodId: string) {
  return invoke<boolean>('favorite_check', { siteKey, vodId })
}

/* ---------------- 观看历史（SQLite） ---------------- */
// 后端语义：每个「站点 + 影片」只保留一行（新集覆盖旧行），list / upsert / set_settings 后自动裁剪

// 读取全部历史（按 updatedAt 倒序；读取前后端按保留策略裁剪一次）
export function historyList() {
  return invoke<HistoryItem[]>('history_list')
}
// 写入/更新一条观看记录（按 siteKey + id 合并），返回更新后的完整列表
export function historyUpsert(item: HistoryItem) {
  return invoke<HistoryItem[]>('history_upsert', { item })
}
// 删除某条记录（按站点 + 影片，epIndex 不参与），返回更新后的完整列表
export function historyRemove(siteKey: string, id: string) {
  return invoke<HistoryItem[]>('history_remove', { siteKey, id })
}
// 清空全部历史，返回空列表
export function historyClear() {
  return invoke<HistoryItem[]>('history_clear')
}
// 读取保留策略：retentionDays=0 永久，maxItems=0 不限
export function historySettings() {
  return invoke<HistorySettings>('history_settings')
}
// 保存保留策略（保存后立即裁剪），返回生效后的策略
export function historySetSettings(retentionDays: number, maxItems: number) {
  return invoke<HistorySettings>('history_set_settings', { retentionDays, maxItems })
}

/* ---------------- 下载（Rust 下载引擎；事件 download-progress 载荷 = 单个 DownloadTask） ---------------- */

// 读取全部下载任务（createdAt 倒序）
export function downloadList() {
  return invoke<DownloadTask[]>('download_list')
}
// 新建下载任务：前端先解析出直链，再交给后端下载引擎
export function downloadStart(params: DownloadStartPayload) {
  return invoke<DownloadTask>('download_start', { params })
}
// 查询该集是否已有可用本地成品（completed 且文件仍存在）：详情 / 历史起播优先本地播放
export function downloadLocalFile(info: DownloadFileInfo) {
  return invoke<DownloadTask | null>('download_local_file', { info })
}
// 任务控制：pause / resume / cancel / retry，返回覆盖后的完整列表
export function downloadControl(id: string, action: DownloadAction) {
  return invoke<DownloadTask[]>('download_control', { id, action })
}
// 删除任务记录（removeFile=false 时保留已下载文件），返回覆盖后的完整列表
export function downloadRemove(id: string, removeFile = false) {
  return invoke<DownloadTask[]>('download_remove', { id, removeFile })
}
// 下载/缓存目录设置：dir 当前目录，defaultDir 系统默认（文档/VBox）
export function downloadSettings() {
  return invoke<DownloadSettings>('download_settings')
}
// 修改下载目录（后端会做可写性校验），返回生效后的设置
export function downloadSetDir(dir: string) {
  return invoke<DownloadSettings>('download_set_dir', { dir })
}
// 播放时自动保存开关；返回生效后的设置
export function downloadSetAutoSave(enabled: boolean) {
  return invoke<DownloadSettings>('download_set_auto_save', { enabled })
}
// 下载并发数 / 全局限速（KB/s，0=不限）；返回生效后的设置
export function downloadSetNetwork(concurrency: number, speedLimitKbps: number) {
  return invoke<DownloadSettings>('download_set_network', { concurrency, speedLimitKbps })
}
// 编辑订阅名称/链接（同步更新启用引用），返回最新订阅列表
export function updateSubscription(oldUrl: string, name: string, url: string) {
  return invoke<Subscription[]>('update_subscription', { oldUrl, name, url })
}
// 导出订阅到 JSON 文件；取消返回 null
export function exportSubscriptions() {
  return invoke<string | null>('export_subscriptions')
}
// 从 JSON 文件导入订阅；取消返回 null
export function importSubscriptions() {
  return invoke<ImportResult | null>('import_subscriptions')
}
// 订阅设置（定时刷新间隔）
export function subscriptionPrefs() {
  return invoke<SubscriptionPrefs>('subscription_prefs')
}
// 保存定时刷新间隔（分钟，0=关闭）
export function setSubscriptionRefresh(minutes: number) {
  return invoke<SubscriptionPrefs>('set_subscription_refresh', { minutes })
}
// 源健康度：列出站点/线路的连续失败记录
export function sourceHealthList() {
  return invoke<SourceHealth[]>('source_health_list')
}
// 源健康度：记录一次结果（前端记录解析/搜索等失败）
export function sourceHealthRecord(
  kind: string,
  key: string,
  label: string,
  ok: boolean,
  error?: string,
) {
  return invoke<SourceHealth>('source_health_record', { kind, key, label, ok, error })
}
// 源健康度：重置单个源（清零连续失败）
export function sourceHealthReset(kind: string, key: string) {
  return invoke<void>('source_health_reset', { kind, key })
}
// 源健康度：全部重置
export function sourceHealthResetAll() {
  return invoke<void>('source_health_reset_all')
}

// 站点 api 支持画像（REA 产物；站点选择器据此区分原生实现/待移植/不支持）
export function siteApiSupport() {
  return invoke<import('@/types').SiteApiSupport[]>('site_api_support')
}
// 源健康度：按 catvod 稳定码聚合失败（错误码分布）
export function sourceHealthCodeStats() {
  return invoke<import('@/types').ErrorCodeStat[]>('source_health_code_stats')
}
// 最近失败事件（倒序）
export function sourceHealthRecentEvents(limit = 20) {
  return invoke<import('@/types').ErrorEvent[]>('source_health_recent_events', { limit })
}
// 最近 N 天成败趋势
export function sourceHealthDaily(days = 7) {
  return invoke<import('@/types').DailyStat[]>('source_health_daily', { days })
}
// 自动保存任务策略：低优先级让路 / 单独限速；返回生效后的设置
export function downloadSetAutoSavePolicy(lowPriority: boolean, autoSaveLimitKbps: number) {
  return invoke<DownloadSettings>('download_set_auto_save_policy', { lowPriority, autoSaveLimitKbps })
}
// 原生「选择文件夹」对话框；用户取消返回 null
export function downloadPickDir() {
  return invoke<string | null>('download_pick_dir')
}
// 打开当前下载目录（资源管理器 / Finder）
export function downloadOpenDir() {
  return invoke<void>('download_open_dir')
}
// 打开某个任务文件所在位置（资源管理器 / Finder 中选中该文件）
export function downloadOpenFileLocation(id: string) {
  return invoke<void>('download_open_file_location', { id })
}
// 下载目录/缓存空间总览 + 磁盘余量
export function downloadStorageStatus() {
  return invoke<DownloadStorageStatus>('download_storage_status')
}
// 清理 .part 残留
export function downloadCleanupParts() {
  return invoke<DownloadCleanupResult>('download_cleanup_parts')
}
// 清理已完成下载（删除文件 + 记录）
export function downloadCleanupCompleted() {
  return invoke<DownloadCleanupResult>('download_cleanup_completed')
}

/* ---------------- Python 运行时 ---------------- */

// 当前 Python 解释器 + 常用依赖状态
export function pythonStatus() {
  return invoke<PythonStatus>('python_status')
}
// 保存解释器路径（空字符串=自动探测）并立即重新检测
export function setPythonPath(path?: string | null) {
  return invoke<PythonStatus>('set_python_path', { path: path || null })
}
// 清空探测缓存并重新检测（不修改已保存路径）
export function refreshPythonStatus() {
  return invoke<PythonStatus>('refresh_python_status')
}

/* ---------------- JVM（jar 蜘蛛）---------------- */

// 运行时状态 + 宿主池 + 覆盖口径
export function jvmStatus() {
  return invoke<JvmStatus>('jvm_status')
}
// 清理 dex 转换缓存（不含加固壳载荷），返回释放字节数
/** 正在进行的 dex→jar 转换进度（设置页轮询）。 */
export function jvmConvertProgress() {
  return invoke<JvmConvertSnapshot>('jvm_convert_progress')
}

export function jvmClearCache() {
  return invoke<number>('jvm_clear_cache')
}

/* ---------------- 兼容性遥测（L1，本机 spool，默认不上传）---------------- */

export function telemetryStatus() {
  return invoke<TelemetryStatus>('telemetry_status')
}
// 导出诊断包（单 JSON），返回文件绝对路径
export function telemetryExport() {
  return invoke<string>('telemetry_export')
}
// 生成预填内容的 GitHub Issue 链接（浏览器打开即可反馈）
export function telemetryIssueUrl() {
  return invoke<string>('telemetry_issue_url')
}
// 清空本机 spool，返回释放字节数
export function telemetryClear() {
  return invoke<number>('telemetry_clear')
}

/* ---------------- Node / drpy 运行时 ---------------- */

// 当前 Node.js 解释器状态
export function nodeStatus() {
  return invoke<NodeStatus>('node_status')
}
// 保存 Node 路径（空字符串=自动探测）并立即重新检测
export function setNodePath(path?: string | null) {
  return invoke<NodeStatus>('set_node_path', { path: path || null })
}
// 清空 Node 探测缓存并重新检测（不修改已保存路径）
export function refreshNodeStatus() {
  return invoke<NodeStatus>('refresh_node_status')
}

/* ---------------- 窗口外观 ---------------- */

// 窗口是否已应用原生毛玻璃（Acrylic / Blur），前端据此开启半透明外壳
export function windowBackdrop() {
  return invoke<boolean>('window_backdrop')
}

/* ---------------- 本地媒体 ---------------- */

// 本地媒体 HTTP 地址（支持 Range，供 mpv 按需读取）
export function localMediaUrl(path: string) {
  return invoke<string>('local_media_url', { path })
}
