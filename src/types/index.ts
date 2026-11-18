// 前端共享类型定义：字段与后端 serde 序列化键严格对齐（原 .aspirecode/sdd/v0.1.0/data-model.md 已归档，见 docs/归档/归档摘要-aspirecode-workbuddy-2026-11-06.md）
// 注意：Site 的源类型字段 JSON 键是 `type`（Rust 字段名是 kind）；Category 的键是 snake_case。

/* ---------------- 订阅 / 站点配置 ---------------- */

export interface Category {
  type_id: string
  type_name: string
}

export interface Site {
  key: string
  name: string
  /** 0=XML、1=苹果CMS、3=Spider/drpy、4=HTTP 远程接口；其它值暂未适配 */
  type: number
  api: string
  /** 是否允许搜索（后端缺省 true） */
  searchable?: boolean
  /** 是否允许快速搜索（后端缺省 true） */
  quickSearch?: boolean
  /** 是否允许切换线路（后端缺省 true） */
  changeable?: boolean
  /** 是否允许筛选（jun 中为死字段，保留兼容） */
  filterable?: boolean
  /** type0/1 的播放页前缀（playUrl） */
  playerUrl?: string
  /** 站点扩展参数（JSON 对象 / 数组 / 字符串均可） */
  ext?: unknown
  /** 站点级 spider jar（http / assets:// / file:// / clan://） */
  jar?: string
  /** 1=IJK、2=Exo、10=外部播放器、-1=跟随全局 */
  playerType?: number
  /** 站点级播放信息超时（秒），0=默认 15 */
  timeout?: number
  /** 嗅探自动点击脚本 `host;js` */
  click?: string
  /** 首页展示风格 */
  style?: string
  categories?: Category[]
  /** @deprecated 仅旧 mock 数据使用；后端不返回该字段（用 supported 判断见 store） */
  playable?: boolean
}

export interface WarehouseUrl {
  url: string
  name?: string | null
}

export interface Parser {
  name: string
  /** 0=嗅探、1=解析接口、4=超解析（后端 Serde 的 kind 序列化为 type） */
  type: number
  url: string
  /** ext：flag 白名单 / header 等（对象 / 数组 / 字符串） */
  ext?: unknown
}

export interface LiveSource {
  name?: string
  /** 0=订阅文本（txt/m3u）、3=spider（.js/.py/jar） */
  type?: number
  /** type0 的订阅地址 */
  url?: string
  /** type3 的 spider api */
  api?: string
  ext?: unknown
  jar?: string
  epg?: string | null
  ua?: string
  header?: Record<string, string> | null
  playerType?: number
  timeout?: number
  logo?: string
}

/** jun 的 rules[]：嗅探 / 广告 / 点击脚本 */
export interface TvRule {
  host?: string | null
  hosts?: string[]
  rule?: string[]
  filter?: string[]
  regex?: string[]
  script?: string[]
}

/** 上游 HTTP/SOCKS 代理规则 */
export interface ProxyRule {
  name?: string
  hosts?: string[]
  urls?: string[]
}

/** 自定义 DNS（`host=ip`） */
export interface Doh {
  name?: string
  url?: string
}

export interface TvConfig {
  sites?: Site[]
  lives?: LiveSource[]
  parses?: Parser[]
  /** VIP 解析白名单 flag */
  flags?: string[]
  /** 自定义 DNS 映射：["host=ip", ...] */
  hosts?: string[]
  proxy?: ProxyRule[]
  rules?: TvRule[]
  doh?: Doh[]
  /** 追加广告域名 */
  ads?: string[]
  /** 顶层 spider jar（可带 `;md5;hash` / `img+` 包装） */
  spider?: string | null
  /** 是否缓存 spider jar，缺省 true */
  jarCache?: boolean
  wallpaper?: string
  danmaku?: string
  urls?: WarehouseUrl[]
}

export interface Subscription {
  name: string
  url: string
}

/** 线路（get_addresses 的 lines 项） */
export interface ConfigLine {
  name?: string | null
  url: string
}

/** 订阅形态：单配置 / 线路合集（多仓） */
export type ConfigKind = 'single' | 'multi'

/** get_addresses 返回：订阅形态 + 线路列表 + 是否命中缓存 */
export interface ConfigInspection {
  kind: ConfigKind
  lines: ConfigLine[]
  cacheHit: boolean
}

/** 当前激活线路的完整加载结果（get_loaded / set_enabled / reload_enabled / restore_enabled） */
export interface LoadedConfig {
  /** 用户原始输入（合集入口时是合集 URL；单配置时是自身） */
  input: string
  /** 当前激活线路所属的订阅/合集入口 URL；单配置时等于 input */
  sourceUrl: string
  /** 实际请求地址（已去除 ;pk;） */
  requestUrl: string
  /** 当前激活线路 URL；单配置时等于 input */
  activeLine: string
  kind: ConfigKind
  /** 线路列表；单配置时只有一项 */
  lines: ConfigLine[]
  /** 当前线路解析出的真实配置 */
  config: TvConfig
  /** 解包格式：plain / base64-marker / aes-cbc-2423 / aes-ecb-pk */
  format: string
  cacheHit: boolean
}

/** @deprecated 旧演示数据/旧接口的地址形态；真实命令返回 ConfigInspection.lines */
export interface Address {
  name: string
  url: string
}

/** @deprecated 仅 mock/旧调用方使用；新代码用 ConfigInspection */
export interface AddressGroup {
  kind: ConfigKind
  addresses: Address[]
}

export interface AddResult {
  subscriptions: Subscription[]
  enabled: string | null
  /** 首个订阅/自动启用时返回的新加载结果 */
  loaded?: LoadedConfig | null
  /** @deprecated 仅演示 mock 兼容字段（真实命令返回 loaded） */
  config?: TvConfig | null
}

/** 顶部栏「仓库」下拉的可选项（多仓时才有） */
export interface WarehouseOption {
  name: string
  url: string
  active: boolean
}

/* ---------------- 点播内容 ---------------- */

export interface VideoItem {
  id: string
  name: string
  cover?: string | null
  remark?: string | null
  /** 来源站点名：搜索页多站点聚合时用于卡片上标注该条来自哪个站点（其它列表为空） */
  siteName?: string
}

export interface Episode {
  title: string
  url: string
}

export interface EpisodeGroup {
  group: string
  items: Episode[]
}

export interface VideoDetail {
  id: string
  name: string
  cover?: string | null
  desc?: string | null
  year?: string | null
  area?: string | null
  director?: string | null
  actor?: string | null
  episodes: EpisodeGroup[]
}

export interface Subtitle {
  lang: string
  url: string
}

export interface PlayResult {
  /** 最终直链，或需要 WebView 嗅探的播放页 URL（sniff=true） */
  url: string
  headers: Record<string, string>
  subtitles: Subtitle[]
  /** 当前地址是否还需要 WebView 抓包才能得到真实直链（后端总是返回；mock 可缺省） */
  sniff?: boolean
  /** 命中的解析器名称 */
  parseName?: string | null
  /** 解析来源（超解析竞速时用于提示） */
  jxFrom?: string | null
  /** 解析过程（抓包/排障面板展示） */
  trace?: string[]
}

/* ---------------- 豆瓣首页（点播首页信息源） ---------------- */

/** 豆瓣条目（首页轮播 / 分类卡片共用；字段由后端 normalize） */
export interface DoubanItem {
  id: string
  title: string
  cover?: string | null
  /** 评分；缺省 / null 表示暂无评分 */
  rating?: number | null
  year?: string | null
  /** 一句话信息：地区 / 类型 / 导演 / 主演 */
  info?: string | null
  /** 剧情简介（轮播项才有） */
  intro?: string | null
  /** 豆瓣条目类型：movie / tv / show 等 */
  subtype?: string | null
}

/** 豆瓣首页的一个分类（热门电影 / 热门剧集 / ...） */
export interface DoubanCollection {
  key: string
  name: string
  items: DoubanItem[]
}

/** 豆瓣首页数据（后端 douban_home 命令 / 浏览器演示 mock） */
export interface DoubanHome {
  carousel: DoubanItem[]
  collections: DoubanCollection[]
  /** 数据抓取时间（毫秒时间戳） */
  fetchedAt: number
  /** 是否命中后端缓存（10 分钟） */
  cacheHit?: boolean
  /**
   * 图片代理前缀（形如 `http://127.0.0.1:1234/img?url=`）。
   * 豆瓣封面直连需要 Referer，WebView 里会被 418，必须经本机代理加载。
   */
  imageProxy?: string | null
}

/* ---------------- 播放器（libmpv 原生子窗口） ---------------- */

/**
 * 播放器状态机（后端 `player_state` / `player-state` 事件）。
 * 视频画面由后端原生子窗口渲染（位于 WebView2 下层），前端只负责上报位置与浮层 UI。
 */
export type PlayerStatus =
  | 'idle'
  | 'opening'
  | 'playing'
  | 'paused'
  | 'buffering'
  | 'ended'
  | 'error'

/** 后端 `player_state` 返回 / `player-state` 事件载荷（camelCase） */
export interface PlayerState {
  status: PlayerStatus
  /** 当前打开的媒体地址（idle 时为空串） */
  url: string
  /** 当前播放进度（秒） */
  positionSec: number
  /** 总时长（秒）；0 表示未知 */
  durationSec: number
  paused: boolean
  buffering: boolean
  /** 音量 0-100 */
  volume: number
  muted: boolean
  /** 播放倍速 */
  rate: number
  /** 视频原始宽高（像素，0 表示未知） */
  width: number
  height: number
  /** 当前播放位置之后已缓冲/缓存的时长（秒）；0 表示未知或无缓存 */
  cacheSec: number
  /** 当前网络缓冲/下载速度（字节/秒）；0 表示未知 */
  bufferSpeedBps: number
  /** 是否已有真实视频帧输出（false 时前端不挖洞，避免缓冲期看到透明窗口） */
  videoReady: boolean
  /** 片源帧率（0 表示未知） */
  containerFps: number
  /** 估算视频帧率（0 表示未知） */
  estimatedFps: number
  /** 当前硬件解码器（空串 / 'no' 表示软解） */
  hwdecCurrent: string
  /** 显示器刷新率（0 表示未知） */
  displayFps: number
  /** 累计丢帧数（排障参考） */
  frameDrops: number
  /** 解码器丢帧数（排障参考） */
  decoderFrameDrops: number
  /** 可选音轨（来自 mpv track-list） */
  audioTracks: PlayerTrack[]
  /** 可选字幕轨（来自 mpv track-list） */
  subtitleTracks: PlayerTrack[]
  /** 当前音轨 id（'no' = 关闭） */
  activeAudioId: string
  /** 当前字幕轨 id（'no' = 关闭） */
  activeSubtitleId: string
  /** 失败原因（status=error 时） */
  error: string
}

/** 播放器可选轨道（字幕 / 音轨）：由后端 mpv `track-list` 枚举 */
export interface PlayerTrack {
  /** 轨道标识（选择时回传，与后端 mpv track id 对应） */
  id: string
  /** 展示名，如「简体中文」「English」「音轨 2」 */
  label: string
  /** 语言标记（可选，展示在名称右侧） */
  lang?: string
  /** 编码格式（可选，如 aac / subrip） */
  codec?: string
  /** 是否外挂字幕/音轨 */
  external?: boolean
}

/** 播放设置（持久化到 <app_data_dir>/player.json） */
export interface PlaybackPrefs {
  /** 硬件解码：auto-safe / d3d11va / auto / no */
  hwdec: string
  /** 上次退出时的音量 0-100（由播放器在关闭播放 / 退出应用时写回，设置页不再编辑） */
  volume: number
  /** 默认倍速 0.25-4 */
  rate: number
  /** 画质增强（Anime4K 超分）：off / lite / standard / strong（播放器页面开关） */
  enhance?: string
  /** 设置文件版本（后端迁移旧版「默认音量」用，前端无需关心） */
  version?: number
}

/* ---------------- 播放缓存（HLS 边下边播） ---------------- */

/** 播放缓存保留策略（持久化到 <app_data_dir>/playback-cache.json） */
export interface PlaybackCachePrefs {
  /** 缓存总大小上限（字节）；0 = 不限制 */
  maxBytes: number
  /** 保留天数；0 = 不限制 */
  retentionDays: number
}

/** 播放缓存当前占用 + 生效策略（设置页展示） */
export interface PlaybackCacheStatus {
  /** 缓存根目录 */
  dir: string
  /** 当前占用字节数 */
  bytes: number
  /** 当前文件数 */
  files: number
  /** 生效的大小上限（0 = 不限制） */
  maxBytes: number
  /** 生效的保留天数（0 = 不限制） */
  retentionDays: number
}

/* ---------------- 运行日志 ---------------- */

/** 一条运行日志（后端 logs_recent） */
export interface LogEntry {
  /** Unix 毫秒时间戳 */
  at: number
  /** info / warn / error */
  level: string
  text: string
}

/* ---------------- 解析进度（parse-progress 全局事件） ---------------- */

/** 解析/抓包阶段：准备 → 解析器 / 解析接口 / 超解析 → 抓包 → 完成 / 失败
 *  （`done` 目前为前端保留态，后端暂不 emit；整次成功/失败以 invoke 结果为准） */
export type ParsePhase = 'prepare' | 'parser' | 'request' | 'super' | 'sniff' | 'done' | 'error'

/** 单条进度的状态：开始 / 成功 / 失败 / 补充信息 */
export type ParseProgressStatus = 'start' | 'ok' | 'fail' | 'info'

/**
 * 后端 `parse-progress` 全局事件载荷（camelCase）。
 * 事件顺序不保证、也不保证全部出现，前端必须容错（只做追加展示，不依赖严格时序）。
 */
export interface ParseProgressEvent {
  /** 前端生成的请求 id：用于过滤「本次解析」的事件（多集/重试时不串台） */
  requestId: string
  /** 阶段 */
  phase: ParsePhase
  /** 状态 */
  status: ParseProgressStatus
  /** 中文短句，如「尝试解析器 量子」「捕获到媒体地址」 */
  title: string
  /** 可选补充：URL、错误、候选数等（可能很长，UI 需截断/折行） */
  detail?: string | null
  /** 后端视角：自本次解析开始的毫秒数（前端用它对齐本地计时） */
  elapsedMs: number
  /** 事件产生的绝对时间戳（毫秒）；当前 UI 未使用，保留给日志/排障 */
  atMs?: number
}

/* ---------------- 搜索 ---------------- */

export type SearchScope = 'current' | 'all'

/** 搜索命中：带上来源站点，前端据此跳转详情 */
export interface SearchHit extends VideoItem {
  siteKey: string
  siteName: string
}

/** 后端分页搜索结果（`search_page`）：首次检索会缓存 5 分钟，翻页不再重复请求站点 */
export interface SearchPageResult {
  items: SearchHit[]
  /** 本次检索的总命中数 */
  total: number
  /** 1-based 页码 */
  page: number
  pageSize: number
  hasMore: boolean
}

export interface SearchState {
  keyword: string
  scope: SearchScope
  loading: boolean
  error: string
  results: SearchHit[]
  searched: boolean
}

/** 搜索设置（<app_data_dir>/search.json）：跨源搜索线程数 */
export interface SearchPrefs {
  /** 并发请求的站点数（1-32） */
  threads: number
}

/* ---------------- 抓包 / 调试 ---------------- */

/** VBox 自身 HTTP 请求日志（network_log；不是系统级抓包） */
export interface NetworkEvent {
  id: number
  atMs: number
  /** config / source / parse / spider / image / other */
  kind: string
  method: string
  url: string
  status?: number | null
  durationMs?: number | null
  size?: number | null
  error?: string | null
}

/* ---------------- 直播 ---------------- */

/** 频道的单个播放源（后端 LiveUrl） */
export interface LiveUrl {
  url: string
  /** 源名；文本里 `url$源名` 的 `$` 之后部分 */
  name?: string
}

/** 一个直播分组（后端 LiveGroup） */
export interface LiveGroup {
  key: string
  name: string
  /** 密码分组的密码；空串/缺省表示公开分组 */
  password?: string
  /** 组级 EPG 地址（m3u 头 x-tvg-url） */
  epg?: string
  channels?: LiveChannel[]
}

/** 单个直播频道（后端 LiveChannel；字段均为 camelCase） */
export interface LiveChannel {
  name: string
  logo?: string | null
  /** 频道级 EPG 标识：m3u 的 tvg-url，缺省回退 tvg-id */
  epg?: string
  /** 所属组 key（= LiveGroup.key） */
  groupKey?: string
  /** 多源地址（url + 源名），换源时按序取用 */
  urls?: LiveUrl[]
  ua?: string
  header?: Record<string, string>
  format?: string
  referer?: string
  origin?: string
  click?: string
  /** 频道级解析开关（预留） */
  parse?: number
  catchup?: unknown
  /** @deprecated 仅旧 mock 使用；真实频道请用 groupKey + name */
  id?: string
  /** @deprecated 仅旧 mock 使用 */
  nowPlaying?: string | null
  /** @deprecated 仅旧 mock 使用 */
  nextPlaying?: string | null
}

/** UI 用节目（live 页把 XMLTV 时间字符串规范化为毫秒） */
export interface EpgProgram {
  /** XMLTV channel 属性（台标 id / 频道标识，用于前端按频道过滤） */
  channel?: string
  title: string
  start?: number | null
  end?: number | null
}

/** live_epg 命令返回的 XMLTV 原始节目（start/end 为原串，如 `20261028120000 +0800`） */
export interface EpgProgramRaw {
  channel: string
  title: string
  start: string
  end: string
}

/* ---------------- 历史记录 ---------------- */

export interface HistoryItem {
  siteKey: string
  siteName: string
  id: string
  name: string
  cover?: string
  epIndex: number
  epTitle: string
  positionSec: number
  durationSec: number
  updatedAt: number
}

/** 历史记录保留策略：retentionDays=0 永久；maxItems=0 不限条数 */
export interface HistorySettings {
  retentionDays: number
  maxItems: number
}

export type HistoryFilter = 'all' | 'week' | 'earlier'

/* ---------------- 下载 ---------------- */

export type DownloadStatus = 'queued' | 'downloading' | 'paused' | 'completed' | 'failed'

export interface DownloadTask {
  id: string
  name: string
  epTitle: string
  siteKey: string
  /** 站点展示名（后端下载任务保存） */
  siteName?: string
  /** 线路 tab 名（如 hhyun / 夸克网盘；旧数据为空） */
  line?: string
  /** 影片 id（站点内唯一，用于播放/去重） */
  vodId?: string
  /** 全局集序号（0 起始） */
  epIndex?: number
  cover?: string
  status: DownloadStatus
  /** 0 - 100 */
  progress: number
  speedBps?: number
  /** 总字节数（未知时为空） */
  sizeBytes?: number
  /** 已下载字节数 */
  downloadedBytes?: number
  /** 完成后本地文件路径（HLS 为本地 playlist.m3u8） */
  filePath?: string
  /** 资源类型：file=直链文件，hls=m3u8 分片 */
  kind?: 'file' | 'hls'
  /** 0=普通下载；1=低优先级（播放时自动保存） */
  priority?: number
  error?: string
  createdAt: number
  updatedAt?: number
}

export type DownloadAction = 'pause' | 'resume' | 'cancel' | 'retry'

/** 新建下载任务参数（前端先解析直链，再交给后端下载） */
export interface DownloadStartPayload {
  siteKey: string
  siteName?: string
  /** 线路 tab 名（如 hhyun / 夸克网盘） */
  line?: string
  vodId?: string
  epIndex?: number
  /** 影片名 */
  name: string
  /** 单集标题 */
  epTitle: string
  cover?: string
  /** 已解析的直链（mp4 / m3u8 等） */
  url: string
  headers?: Record<string, string>
  /** 0=普通下载；1=低优先级（播放时自动保存） */
  priority?: number
}

/**
 * 「查找该集已下载的本地成品」元信息（与后端 SavedFileInfo 对齐）。
 * 优先按 siteKey + vodId + epIndex 精确匹配；缺 siteKey/vodId 时退化为 name + epTitle。
 */
export interface DownloadFileInfo {
  siteKey: string
  siteName?: string
  /** 线路 tab 名（如 hhyun / 夸克网盘） */
  line?: string
  vodId?: string
  /** 全局集序号（0 起始） */
  epIndex?: number
  /** 影片名 */
  name: string
  epTitle?: string
  cover?: string
}

/** 下载/缓存目录设置：dir 当前目录，defaultDir 系统默认（文档/VBox），autoSave=播放时自动保存 */
export interface DownloadSettings {
  dir: string
  defaultDir: string
  autoSave: boolean
  /** 同时下载任务数（1-8，默认 1） */
  concurrency: number
  /** 全局下载限速 KB/s（0 = 不限速） */
  speedLimitKbps: number
  /** 自动保存任务低优先级（普通下载进行时让路） */
  autoSaveLowPriority: boolean
  /** 自动保存任务单独限速 KB/s（0 = 不额外限制） */
  autoSaveLimitKbps: number
}

/** 下载目录/缓存空间总览（设置页「空间管理」） */
export interface DownloadStorageStatus {
  dir: string
  /** 所在磁盘总容量（0=未知） */
  diskTotalBytes: number
  /** 所在磁盘剩余空间（0=未知） */
  diskFreeBytes: number
  /** 下载目录总占用 */
  usedBytes: number
  /** 播放缓存目录占用（下载目录/cache） */
  cacheBytes: number
  /** .part 残留占用 */
  partBytes: number
  /** 已完成文件总大小（仅统计仍存在的文件） */
  completedBytes: number
  partCount: number
  completedCount: number
}

/** 清理结果 */
export interface DownloadCleanupResult {
  removedFiles: number
  freedBytes: number
}

/* ---------------- 收藏 ---------------- */

/** 收藏的影片（按 siteKey + vodId 唯一） */
export interface FavoriteItem {
  siteKey: string
  /** 站点展示名（收藏时快照，站点移除后仍可展示） */
  siteName?: string
  vodId: string
  name: string
  cover?: string
  year?: string
  area?: string
  remark?: string
  createdAt: number
}

/** 收藏筛选：全部 / 本周收藏 / 更早 */
export type FavoriteFilter = 'all' | 'week' | 'earlier'

/* ---------------- 订阅设置 ---------------- */

/** 订阅相关设置（定时刷新间隔，0 = 关闭） */
export interface SubscriptionPrefs {
  refreshMinutes: number
}

/** 订阅导入结果 */
export interface ImportResult {
  imported: number
  skipped: number
  subscriptions: Subscription[]
}

/* ---------------- 源健康度 ---------------- */

/** 站点/线路的连续失败记录（后端 source_health.db） */
export interface SourceHealth {
  kind: 'site' | 'line'
  /** site key 或 线路 URL */
  key: string
  /** 最近一次记录时的展示名 */
  label: string
  /** 连续失败次数（成功一次清零） */
  failures: number
  /** 累计失败次数 */
  totalFailures: number
  lastError?: string
  /** 最近一次失败识别出的 catvod 稳定码（如 catvod.net.timeout） */
  lastCode?: string | null
  updatedAt: number
  /** 连续失败 >= 3，前端视为禁用 */
  disabled: boolean
}

/* ---------------- 主题 / UI ---------------- */

export type ThemeMode = 'system' | 'light' | 'dark'

/** 一套「配色」在亮/暗两种模式下的主题 class 名（与 styles/theme.scss 对应） */
export interface ThemeHue {
  key: string
  label: string
  light: string
  dark: string
  /** 用于设置页色块展示：主色 / 浅色 / 页面底色 / 深色 */
  colors: string[]
}

export type ToastLevel = 'info' | 'success' | 'warning' | 'error'

export interface ToastMessage {
  id: number
  text: string
  level: ToastLevel
}

export interface UiPersist {
  mode: ThemeMode
  hue: string
  sidebarCollapsed?: boolean
  /** 侧边与标题栏亚克力（默认开启） */
  acrylic?: boolean
  /** 全局背景亚克力（依赖侧边与标题栏开启，默认关闭） */
  acrylicContent?: boolean
}

/* ---------------- 源 tab ---------------- */

/** 源的本机支持级别：local=本机可加载；python=Python 运行时未就绪；jar=需 Android Host（后续）；unsupported=其它 */
export type SiteSupportLevel = 'local' | 'python' | 'jar' | 'unsupported'

/** 站点 api 的 REA 画像摘要（后端 site_api_support 返回，用于站点选择器联动） */
export interface SiteApiSupport {
  api: string
  /** 已有原生实现（规则引擎 / 原生解析家族） */
  native: boolean
  /** rulepack / script / unported / unsupported */
  strategy: string
  class?: string | null
  jars: string[]
  difficulty?: string | null
  reason?: string | null
  blockedReason?: string | null
  /** 阻塞原因的用户可读解释 */
  blockedHint?: string | null
  sites: number
}

/** 一条线路 / 一个站点集合的源能力统计（设置页展示“本机可用 / jar”） */
export interface SiteStats {
  total: number
  local: number
  jar: number
  python: number
  unsupported: number
}

export interface SourceTabItem {
  key: string
  name: string
  type: number
  /** 本机是否可加载（Python 未就绪 / jar 均为 false） */
  supported: boolean
  /** 本机不可用时的原因文案（统一“后续支持 / 暂未适配”措辞） */
  reason?: string
  /** 支持但需要提示的能力范围（例：type3 仅 js spider 子集） */
  note?: string
  /** 该源的支持级别 */
  level?: SiteSupportLevel
}

/* ---------------- Python 运行时 ---------------- */

export interface PythonDependency {
  name: string
  module: string
  available: boolean
}

export interface PythonStatus {
  available: boolean
  label?: string | null
  version?: string | null
  preferred?: string | null
  /** 随应用集成的解释器路径（发布包内置时存在） */
  bundled?: string | null
  error?: string | null
  dependencies: PythonDependency[]
}

export interface NodeStatus {
  available: boolean
  label?: string | null
  version?: string | null
  preferred?: string | null
  /** 随应用集成的 Node 路径（发布包内置时存在） */
  bundled?: string | null
  error?: string | null
}

/* ---------------- 外部工具 FFmpeg ---------------- */

/** FFmpeg 探测结果（后端 `ffmpeg_status`） */
export interface FfmpegStatus {
  /** 用户配置的路径（空串 = 自动） */
  path: string
  /** 实际解析到的可执行文件（配置 / 已下载 / 随包 / 环境变量 / PATH） */
  effective: string
  /** 来源：configured / managed / bundled / env / path */
  source: string
  /** `ffmpeg -version` 是否成功 */
  available: boolean
  /** 版本首行（可用时） */
  version?: string | null
  /** 失败原因（不可用时） */
  error?: string | null
  /** 随安装包集成的副本路径（存在时；发布包一般都有） */
  bundled?: string | null
  /** 已下载集成到本机的副本路径（存在时） */
  managed?: string | null
  /** 是否正在下载集成中 */
  provisioning: boolean
  /** 当前平台是否支持一键下载集成（目前仅 Windows x64） */
  canProvision: boolean
}

/** FFmpeg 一键集成下载进度（后端事件 `ffmpeg-provision`） */
export interface FfmpegProvisionProgress {
  /** download / verify / extract / done / error */
  phase: string
  /** 已下载字节 */
  downloaded: number
  /** 总字节（0 = 未知） */
  total: number
  /** 给用户看的一句话 */
  message: string
}

/* ---------------- 在线字幕（弹窗搜索） ---------------- */

/** 在线字幕源（后端 `subtitle_sources`） */
export interface SubtitleSource {
  /** 源 id（搜索/下载时回传，如 "subhd"） */
  id: string
  /** 展示名，如 "SubHD" */
  name: string
  /** 一句话说明 */
  hint: string
}

/** 一条在线字幕搜索结果（后端 `subtitle_search`） */
export interface OnlineSubtitle {
  /** 站内 id（下载时回传） */
  id: string
  /** 源 id */
  source: string
  /** 片名 / 剧名 */
  title: string
  /** 字幕版本名（片源版本、语言、特效说明） */
  name: string
  /** 格式：ASS / SRT / SUP / SSA … */
  format: string
  /** 语言标签：双语 / 简体 / 繁体 / 英语 … */
  languages: string[]
  /** 来源徽标：官方字幕 / 其他来源 / 精选推荐 */
  origin: string
  /** 字幕组（可空） */
  group: string
  /** 文件大小（站点原样，如 "180k"） */
  size: string
  /** 下载次数（站点原样） */
  downloads: string
  /** 发布时间（站点原样） */
  date: string
  /** 发布人 */
  uploader: string
  /** 详情页地址 */
  pageUrl: string
}

/* ---------------- 插件（GLSL 着色器 / lavfi 滤镜） ---------------- */

/** 插件里的一个着色器条目（`contributes.shaders[]`） */
export interface PluginShaderInfo {
  id: string
  name: string
  /** 相对插件目录的 .glsl/.hook 文件 */
  files: string[]
  /** 低 / 中 / 高 */
  gpuLoad: string
  /** 是否允许与内置 Anime4K 链叠加 */
  stackWithBuiltin: boolean
  /** 用途：upscale / hdr / sharpen / repair / style（可为空） */
  category: string
}

/** 插件里的一个 lavfi 滤镜条目（`contributes.videoFilters[]` / `audioFilters[]`） */
export interface PluginFilterInfo {
  id: string
  name: string
  /** 必须是 `lavfi=[...]` 形式 */
  graph: string
  gpuLoad: string
  /** 用途：upscale / hdr / sharpen / repair / style（可为空） */
  category: string
}

/** 插件列表里的一项（后端 `plugin_overview`） */
export interface PluginInfo {
  id: string
  name: string
  version: string
  /** shader / filter / bundle / 空 */
  type: string
  author: string
  description: string
  /** 插件主页（http/https，空串表示未提供） */
  homepage: string
  /** 随包内置插件（启动时自动安装/修复） */
  bundled: boolean
  minAppVersion: string
  enabled: boolean
  /** 启用顺序（小的先应用） */
  order: number
  /** enabled / disabled / error / invalid */
  status: string
  error: string
  shaders: PluginShaderInfo[]
  videoFilters: PluginFilterInfo[]
  audioFilters: PluginFilterInfo[]
  /** 当前选中的着色器预设 id（多预设插件） */
  preset: string
}

/** 插件总览（后端 `plugin_overview` / `plugin_rescan`） */
export interface PluginOverview {
  /** `<app_data_dir>/plugins` 绝对路径 */
  root: string
  /** 安全模式：`VBOX_DISABLE_PLUGINS=1` 时为 true */
  disabled: boolean
  /** 最近一次合成/应用失败的原因（空串表示正常） */
  lastError: string
  plugins: PluginInfo[]
}

/** 按 catvod 稳定码聚合的失败统计（设置页「错误码分布」） */
export interface ErrorCodeStat {
  code: string
  /** 当前仍带该码的源数量 */
  sources: number
  /** 该码对应的累计失败次数 */
  failures: number
}

/** 一次源失败事件（设置页「最近失败事件」） */
export interface ErrorEvent {
  kind: string
  key: string
  label: string
  code: string | null
  /** 原始错误文本（含站点/jar 身份）；历史事件可能为空 */
  message: string | null
  createdAt: number
}

/** 某天的请求成败计数（设置页「近 7 天」趋势） */
export interface DailyStat {
  day: string
  ok: number
  fail: number
}

/* ---------------- 网盘账号登录（W13） ---------------- */

/** 二维码 / 设备码票据（后端安全视图：内部 uid/sign/device_code 不下发前端） */
export interface PanQrTicket {
  ticket_id: string
  /** qr_code = 二维码内容；device_code = 设备码（content 为验证地址） */
  kind: 'qr_code' | 'device_code'
  /** 二维码要编码的 URL，或设备码验证地址 */
  content: string
  /** 二维码图片地址（115 官方接口；夸克/迅雷为空串） */
  image_url: string
  /** 有效期（秒） */
  expires_in: number
  /** 过期时刻（Unix 秒；0 表示未知） */
  expires_at: number
}

/** 账号状态（W14 账号卡片徽标） */
export type PanAccountStatus = 'connected' | 'expired' | 'not_logged_in'

/** 账号摘要：凭据只留在后端，这里只有掩码与状态 */
export interface PanAccountView {
  account_id: string
  provider: string
  display_name: string | null
  /** 账号标识掩码（u***88 / 138****678） */
  masked_id: string | null
  status: PanAccountStatus
  /** 最后更新时间（Unix 秒） */
  updated_at: number
}

/** 登录轮询结果（成功时已落盘加密凭据，只回账号摘要） */
export type PanLoginPollResult =
  | { state: 'waiting' }
  | { state: 'scanned' }
  | { state: 'success'; account: PanAccountView }
  | { state: 'expired' }
  | { state: 'failure'; message: string }

/** 单个 provider 的登录能力 */
export interface PanLoginProvider {
  provider: string
  /** 支持协议登录（login_start / login_poll） */
  protocol: boolean
  /** 有网页登录兜底（open_web_login） */
  webLogin: boolean
}
