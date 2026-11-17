// 云盘聚合搜索（W14）Tauri 命令封装。
// 浏览器演示模式下不可用：调用方先判 `api.demoMode`（见 src/api/index.ts）。
import { invoke } from '@tauri-apps/api/core'

/** 搜索范围（与 vbox_core::cloudsearch::SearchProvider 的 snake_case 值对应） */
export type CloudSearchProvider =
  | 'quark'
  | 'aliyun'
  | 'p115'
  | 'xunlei'
  | 'pikpak'
  | 'uc'
  | 'baidu'
  | 'tianyi'
  | 'mobile'
  | 'p123'
  | 'magnet'
  | 'ed2k'
  | 'other'

export type CloudSortBy = 'relevance' | 'time'

export interface CloudSearchQuery {
  keyword: string
  page?: number
  pageSize?: number
  providers?: CloudSearchProvider[]
  sort?: CloudSortBy
}

export interface CloudSearchItem {
  id: string
  title: string
  provider: CloudSearchProvider
  shareUrl: string
  password?: string | null
  size?: number | null
  publishedAt?: number | null
  sources: string[]
  playable: boolean
}

export interface CloudProviderCount {
  provider: CloudSearchProvider
  count: number
}

export interface CloudSearchPage {
  items: CloudSearchItem[]
  total: number
  partial: boolean
  elapsedMs: number
  degradedSources: string[]
  /** 跨频道去重合并掉的重复条数 */
  deduped: number
  /** 去重后、provider 筛选前的分网盘计数 */
  providerCounts: CloudProviderCount[]
}

export interface CloudSourceStatus {
  kind: 'builtin_kernel' | 'pan_sou'
  ready: boolean
  version: string
  detail: string
}

/** 聚合搜索（内置内核 + 可选增强源） */
export function cloudSearch(query: CloudSearchQuery): Promise<CloudSearchPage> {
  return invoke<CloudSearchPage>('cloud_search', {
    keyword: query.keyword,
    page: query.page ?? 1,
    pageSize: query.pageSize ?? 20,
    providers: query.providers ?? [],
    sort: query.sort ?? 'relevance',
  })
}

/** 首页热搜词 */
export function cloudHotSearches(): Promise<string[]> {
  return invoke<string[]>('cloud_hot_searches')
}

/** 当前搜索源状态（内置内核 + 可选 PanSou 增强源） */
export function cloudSourceStatus(): Promise<CloudSourceStatus[]> {
  return invoke<CloudSourceStatus[]>('cloud_source_status')
}

/** 配置搜索源（仅 PanSou；`url` 传空字符串表示关闭） */
export function cloudSourceSet(payload: {
  kind: 'pansou'
  url?: string | null
  token?: string | null
}): Promise<CloudSourceStatus[]> {
  return invoke<CloudSourceStatus[]>('cloud_source_set', {
    kind: payload.kind,
    url: payload.url ?? '',
    token: payload.token ?? '',
  })
}

/** 保存远端频道清单配置（列表地址 + RSA 公钥 PEM；传空字符串清空，不影响 PanSou） */
export function cloudSourcesConfigSet(payload: {
  sourcesUrl?: string | null
  sourcesPubkey?: string | null
}): Promise<CloudSourceStatus[]> {
  return invoke<CloudSourceStatus[]>('cloud_sources_config_set', {
    sourcesUrl: payload.sourcesUrl ?? '',
    sourcesPubkey: payload.sourcesPubkey ?? '',
  })
}

/** 搜索源自检 */
export function cloudSourceTest(): Promise<string> {
  return invoke<string>('cloud_source_test')
}

/** 远端频道清单热更新：抓取 → RSA 验签 → 缓存 → 重建内核（W14 §11 P3） */
export function cloudSourcesUpdate(url?: string | null): Promise<string> {
  return invoke<string>('cloud_sources_update', {
    url: url && url.trim() ? url.trim() : null,
  })
}

/** 云盘分享解析结果（`provider` 为登录 provider 键；直链仅进程内/播放器使用） */
export interface CloudResolvedPlay {
  provider: string
  fileName: string
  url: string
  headers: Record<string, string>
}

/** 搜索结果 → 播放直链：分享展开 + W13 登录凭据（未登录/需转存时抛可操作错误） */
export function cloudResolvePlay(
  shareUrl: string,
  password?: string | null,
): Promise<CloudResolvedPlay> {
  return invoke<CloudResolvedPlay>('cloud_resolve_play', {
    shareUrl,
    password: password ?? null,
  })
}

/** PanSou Docker 运行环境检测结果（W14 §8.7；不含路径/凭据） */
export interface CloudPansouEnv {
  dockerInstalled: boolean
  dockerVersion: string | null
  daemonRunning: boolean
  imagePresent: boolean
  imageDigest: string | null
  remoteDigest: string | null
  updateAvailable: boolean | null
  pansouConfigured: boolean
  hint: string
  upstream: string
  image: string
  installUrl: string
}

/** 检测 PanSou 的 Docker 运行环境（只读，不启动/修改容器） */
export function cloudPansouEnv(): Promise<CloudPansouEnv> {
  return invoke<CloudPansouEnv>('cloud_pansou_env')
}
