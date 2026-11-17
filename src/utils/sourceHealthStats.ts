/**
 * 源健康的派生统计（纯函数，便于测试与复用）。
 *
 * 后端 `source_health` 以 `kind/key` 记录（site=站点 key、line=线路 URL），
 * 不直接记录 api；这里用配置里的 `Site.api` 把站点 key 映射到 api 再聚合，
 * 用于设置页展示「哪个 api 家族失败最多」。
 */
import type { Site, SourceHealth } from '@/types'

export interface ApiFailureStat {
  /** api 标识（站点 key 未匹配到配置时为 `（未知源）`） */
  api: string
  /** 出现失败记录的站点数 */
  sites: number
  /** 累计失败次数（totalFailures 之和） */
  failures: number
  /** 其中连续失败被禁用的站点数 */
  disabled: number
  /** 最近一次失败识别出的稳定码（多个取次数最多者，缺失为 null） */
  lastCode: string | null
  /** 最近出错的一个站点展示名（便于定位） */
  label: string
}

const UNKNOWN_API = '（未知源）'

export function groupApiFailures(
  items: SourceHealth[],
  sites: Site[],
  limit = 8,
): ApiFailureStat[] {
  const byKey = new Map<string, Site>()
  for (const site of sites) {
    if (site.key) byKey.set(site.key, site)
  }

  const stats = new Map<string, ApiFailureStat>()
  const codes = new Map<string, Map<string, number>>()

  for (const item of items) {
    if (item.kind !== 'site') continue
    if (!item.totalFailures && !item.failures) continue
    const site = byKey.get(item.key)
    const api = site?.api?.trim() || UNKNOWN_API
    const stat = stats.get(api) ?? {
      api,
      sites: 0,
      failures: 0,
      disabled: 0,
      lastCode: null,
      label: '',
    }
    stat.sites += 1
    stat.failures += item.totalFailures || item.failures
    if (item.disabled) stat.disabled += 1
    if (item.label.trim() && !stat.label) stat.label = item.label.trim()
    if (item.lastCode) {
      const bucket = codes.get(api) ?? new Map<string, number>()
      bucket.set(item.lastCode, (bucket.get(item.lastCode) ?? 0) + 1)
      codes.set(api, bucket)
    }
    stats.set(api, stat)
  }

  for (const [api, bucket] of codes) {
    const stat = stats.get(api)
    if (!stat) continue
    let best: string | null = null
    let bestCount = 0
    for (const [code, count] of bucket) {
      if (count > bestCount) {
        best = code
        bestCount = count
      }
    }
    stat.lastCode = best
  }

  return [...stats.values()]
    .sort((a, b) => b.failures - a.failures || b.disabled - a.disabled || a.api.localeCompare(b.api))
    .slice(0, Math.max(1, limit))
}
