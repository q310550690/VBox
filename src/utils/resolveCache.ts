// 解析结果 TTL 缓存（localStorage）：
// - 同一集成功拿到「最终播放地址 + 请求头」后记住一段时间，再次进入该集直接复用，
//   省掉一次 resolve + WebView 抓包（也避免解析结果每次抖动）；
// - 只缓存**已经拿到最终直链**的结果（resolve 可能需要再 sniff，调用方在拿到最终地址后写入）；
// - 带有效期：解析回来的地址可能带时效签名，超过 TTL 一律当未命中重新解析；
// - 失败回退：详情页在播放器报错 / 打开失败时清掉该条并重新解析（见 detail/index.vue）。
const STORAGE_KEY = 'vbox.resolveCache'

/** 缓存有效期：30 分钟（超过后重新解析；地址失效时由详情页失败回退兜底） */
export const RESOLVE_CACHE_TTL_MS = 30 * 60 * 1000
/** 最多保留条数：超出后按写入时间淘汰最旧的 */
const MAX_ENTRIES = 60

export interface CachedResolve {
  /** 最终播放地址（直链 / m3u8） */
  url: string
  /** 播放所需请求头（Referer / UA / Cookie 等） */
  headers: Record<string, string>
  /** 写入时间（Unix 毫秒） */
  at: number
}

export interface ResolveCacheKeyParts {
  /** 站点 key */
  siteKey: string
  /** 影片 id（详情页 id） */
  vodId: string
  /** 线路标识（线路分组名；空 = 默认线路） */
  line: string
  /** 全局集序号（跨线路稳定） */
  epIndex: number
  /** 解析器名（空 = 自动） */
  parseName: string
  /** 该集原始播放页 URL：详情刷新后可能变化，变化后不再复用旧解析结果 */
  epUrl: string
}

/** djb2 字符串哈希：只用于把 epUrl 压短，避免 localStorage 的 key 过长 */
function hashText(text: string): string {
  let hash = 5381
  for (let i = 0; i < text.length; i += 1) {
    hash = ((hash << 5) + hash + text.charCodeAt(i)) >>> 0
  }
  return hash.toString(36)
}

/** 缓存 key：站点 / 影片 / 线路 / 集 / 解析器 / 原始播放页 URL 任一变化都不复用 */
export function resolveCacheKey(parts: ResolveCacheKeyParts): string {
  const line = parts.line.trim() || '-'
  const parseName = parts.parseName.trim() || 'auto'
  return [parts.siteKey, parts.vodId, line, parts.epIndex, parseName, hashText(parts.epUrl)].join('|')
}

/* ---------------- 存储读写（损坏 / 被禁用时静默降级） ---------------- */

function readStore(): Record<string, CachedResolve> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const data = JSON.parse(raw)
    if (!data || typeof data !== 'object' || Array.isArray(data)) return {}
    const store: Record<string, CachedResolve> = {}
    for (const [key, value] of Object.entries(data as Record<string, unknown>)) {
      if (!value || typeof value !== 'object') continue
      const entry = value as Partial<CachedResolve>
      if (typeof entry.url !== 'string' || !entry.url) continue
      if (!Number.isFinite(entry.at)) continue
      store[key] = {
        url: entry.url,
        headers:
          entry.headers && typeof entry.headers === 'object'
            ? (entry.headers as Record<string, string>)
            : {},
        at: Number(entry.at),
      }
    }
    return store
  } catch {
    return {}
  }
}

function writeStore(store: Record<string, CachedResolve>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store))
  } catch {
    /* localStorage 不可用 / 超限：静默忽略，退化为不缓存 */
  }
}

/** 读取未过期的解析结果；过期条目顺手清掉 */
export function getResolveCache(key: string): CachedResolve | null {
  if (!key) return null
  const store = readStore()
  const entry = store[key]
  if (!entry) return null
  if (Date.now() - entry.at > RESOLVE_CACHE_TTL_MS) {
    delete store[key]
    writeStore(store)
    return null
  }
  return entry
}

/** 写入解析结果（同 key 覆盖；超过上限按写入时间淘汰最旧） */
export function setResolveCache(
  key: string,
  url: string,
  headers: Record<string, string> | null | undefined,
): void {
  if (!key || !url) return
  const store = readStore()
  store[key] = { url, headers: { ...(headers ?? {}) }, at: Date.now() }
  const keys = Object.keys(store)
  if (keys.length > MAX_ENTRIES) {
    keys
      .sort((a, b) => store[a].at - store[b].at)
      .slice(0, keys.length - MAX_ENTRIES)
      .forEach((stale) => delete store[stale])
  }
  writeStore(store)
}

/** 删除单条（播放失败回退 / 手动重试时用） */
export function clearResolveCache(key: string): void {
  if (!key) return
  const store = readStore()
  if (!(key in store)) return
  delete store[key]
  writeStore(store)
}

/** 清空全部解析缓存 */
export function clearAllResolveCache(): void {
  try {
    localStorage.removeItem(STORAGE_KEY)
  } catch {
    /* 忽略 */
  }
}
