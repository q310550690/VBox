// 观看历史 mock（仅浏览器预览使用）：localStorage 持久化，语义与 Rust SQLite 后端对齐：
//   - 每个「站点 + 影片」只保留一行（新集覆盖旧行）
//   - 列表按 updatedAt 倒序
//   - 保留策略 retentionDays（0=永久）/ maxItems（0=不限），写入 / 改设置时自动裁剪
import type { HistoryItem, HistorySettings } from '@/types'

const STORAGE_KEY = 'vbox.history.demo.v3'
const SETTINGS_KEY = 'vbox.history.settings'
const DAY_MS = 24 * 60 * 60 * 1000

/** 默认保留策略（与后端一致）：永久保存、最多 200 条 */
export const DEFAULT_HISTORY_SETTINGS: HistorySettings = {
  retentionDays: 0,
  maxItems: 200,
}

const DEMO_HISTORY: HistoryItem[] = [
  {
    siteKey: 'demo_cms_cuiwei',
    siteName: '演示 · 翠微影视',
    id: 'demo-1-1-0',
    name: '长夜将明',
    cover: 'https://picsum.photos/seed/vbox-1-0/300/450',
    epIndex: 2,
    epTitle: '第3集',
    positionSec: 750,
    durationSec: 2160,
    updatedAt: Date.now() - 1000 * 60 * 60 * 5,
  },
  {
    siteKey: 'demo_cms_shuangyue',
    siteName: '演示 · 霜月剧场',
    id: 'demo-2-1-3',
    name: '雾城疑云',
    cover: 'https://picsum.photos/seed/vbox-2-3/300/450',
    epIndex: 7,
    epTitle: '第8集',
    positionSec: 1320,
    durationSec: 2640,
    updatedAt: Date.now() - 1000 * 60 * 60 * 30,
  },
  {
    siteKey: 'demo_cms_cuiwei',
    siteName: '演示 · 翠微影视',
    id: 'demo-1-2-1',
    name: '盛夏回声',
    cover: 'https://picsum.photos/seed/vbox-1-19/300/450',
    epIndex: 0,
    epTitle: '第1集',
    positionSec: 180,
    durationSec: 7200,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 8,
  },
  {
    siteKey: 'demo_cms_cuiwei',
    siteName: '演示 · 翠微影视',
    id: 'demo-1-1-4',
    name: '流浪星球',
    cover: 'https://picsum.photos/seed/vbox-1-4/300/450',
    epIndex: 5,
    epTitle: '第6集',
    positionSec: 960,
    durationSec: 2400,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 12,
  },
  {
    siteKey: 'demo_cms_shuangyue',
    siteName: '演示 · 霜月剧场',
    id: 'demo-2-2-2',
    name: '风起长安',
    cover: 'https://picsum.photos/seed/vbox-2-21/300/450',
    epIndex: 11,
    epTitle: '第12集',
    positionSec: 300,
    durationSec: 2700,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 18,
  },
  {
    siteKey: 'demo_cms_yanshan',
    siteName: '演示 · 烟山放映室',
    id: 'demo-3-1-8',
    name: '无声告白',
    cover: 'https://picsum.photos/seed/vbox-3-8/300/450',
    epIndex: 1,
    epTitle: '第2集',
    positionSec: 1980,
    durationSec: 2700,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 25,
  },
  {
    siteKey: 'demo_cms_qinghe',
    siteName: '演示 · 青禾影库',
    id: 'demo-2-1-0-72',
    name: '山海之间',
    cover: 'https://picsum.photos/seed/vbox-2-72/300/450',
    epIndex: 3,
    epTitle: '第4集',
    positionSec: 1080,
    durationSec: 2640,
    updatedAt: Date.now() - 1000 * 60 * 60 * 2,
  },
  {
    siteKey: 'demo_cms_lanxi',
    siteName: '演示 · 澜溪剧场',
    id: 'demo-3-2-5-70',
    name: '赛博旧梦',
    cover: 'https://picsum.photos/seed/vbox-3-70/300/450',
    epIndex: 8,
    epTitle: '第9集',
    positionSec: 2100,
    durationSec: 3000,
    updatedAt: Date.now() - 1000 * 60 * 60 * 9,
  },
  {
    siteKey: 'demo_cms_muxi',
    siteName: '演示 · 暮汐资源',
    id: 'demo-1-1-12-13',
    name: '光年之外',
    cover: 'https://picsum.photos/seed/vbox-1-13/300/450',
    epIndex: 0,
    epTitle: '正片',
    positionSec: 600,
    durationSec: 7800,
    updatedAt: Date.now() - 1000 * 60 * 60 * 20,
  },
  {
    siteKey: 'demo_cms_haijiao',
    siteName: '演示 · 海角影院',
    id: 'demo-4-2-2-58',
    name: '灵笼',
    cover: 'https://picsum.photos/seed/vbox-4-58/300/450',
    epIndex: 6,
    epTitle: '第7集',
    positionSec: 480,
    durationSec: 1560,
    updatedAt: Date.now() - 1000 * 60 * 60 * 46,
  },
  {
    siteKey: 'demo_cms_xingchen',
    siteName: '演示 · 星宸影视',
    id: 'demo-1-3-4-53',
    name: '长安三万里',
    cover: 'https://picsum.photos/seed/vbox-1-53/300/450',
    epIndex: 0,
    epTitle: '正片',
    positionSec: 3600,
    durationSec: 10080,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 4,
  },
  {
    siteKey: 'demo_cms_cangshan',
    siteName: '演示 · 苍山点播',
    id: 'demo-3-1-9-79',
    name: '中国奇谭',
    cover: 'https://picsum.photos/seed/vbox-3-79/300/450',
    epIndex: 2,
    epTitle: '第3集',
    positionSec: 300,
    durationSec: 1500,
    updatedAt: Date.now() - 1000 * 60 * 60 * 24 * 9,
  },
]

/** 非负整数归一：缺省用默认值，负数 / NaN 按 0 处理（与后端一致） */
function normalizeCount(value: unknown, fallback: number): number {
  if (value === undefined || value === null || value === '') return fallback
  const n = Number(value)
  if (!Number.isFinite(n)) return 0
  return Math.max(0, Math.floor(n))
}

/** 列表按 updatedAt 倒序（updatedAt 相同按原顺序稳定排序） */
function sortByUpdatedAt(items: HistoryItem[]): HistoryItem[] {
  return [...items].sort((a, b) => (b.updatedAt || 0) - (a.updatedAt || 0))
}

/** 同一「站点 + 影片」只留最新一行（兼容旧版按集存储的 localStorage 数据） */
function dedupeByFilm(items: HistoryItem[]): HistoryItem[] {
  const seen = new Set<string>()
  const next: HistoryItem[] = []
  for (const item of sortByUpdatedAt(items)) {
    const key = `${item.siteKey}\u0000${item.id}`
    if (seen.has(key)) continue
    seen.add(key)
    next.push(item)
  }
  return next
}

/** 按保留策略裁剪（先按天数，再按条数） */
function prune(items: HistoryItem[], settings: HistorySettings): HistoryItem[] {
  let next = sortByUpdatedAt(items)
  if (settings.retentionDays > 0) {
    const cutoff = Date.now() - settings.retentionDays * DAY_MS
    next = next.filter((it) => (it.updatedAt || 0) >= cutoff)
  }
  if (settings.maxItems > 0) next = next.slice(0, settings.maxItems)
  return next
}

function safeParseItems(raw: string | null): HistoryItem[] | null {
  if (!raw) return null
  try {
    const data = JSON.parse(raw)
    return Array.isArray(data) ? (data as HistoryItem[]) : null
  } catch {
    return null
  }
}

/** 把任意对象收敛成合法保留策略 */
function normalizeSettings(value: unknown): HistorySettings {
  const source = (value ?? {}) as Partial<HistorySettings>
  return {
    retentionDays: normalizeCount(source.retentionDays, DEFAULT_HISTORY_SETTINGS.retentionDays),
    maxItems: normalizeCount(source.maxItems, DEFAULT_HISTORY_SETTINGS.maxItems),
  }
}

/* ---------------- 保留策略 ---------------- */

export function loadHistorySettings(): HistorySettings {
  let raw: string | null = null
  try {
    raw = localStorage.getItem(SETTINGS_KEY)
  } catch {
    return { ...DEFAULT_HISTORY_SETTINGS }
  }
  if (!raw) return { ...DEFAULT_HISTORY_SETTINGS }
  try {
    return normalizeSettings(JSON.parse(raw))
  } catch {
    return { ...DEFAULT_HISTORY_SETTINGS }
  }
}

function writeHistorySettings(settings: HistorySettings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings))
  } catch {
    /* 存储不可用时忽略 */
  }
}

/** 保存保留策略，并立即按新策略裁剪现有记录 */
export function saveHistorySettings(patch: Partial<HistorySettings>): HistorySettings {
  const next = normalizeSettings({ ...loadHistorySettings(), ...patch })
  writeHistorySettings(next)
  saveHistory(prune(loadHistory(), next))
  return next
}

/* ---------------- 历史记录 ---------------- */

export function loadHistory(): HistoryItem[] {
  const settings = loadHistorySettings()
  const saved = safeParseItems(localStorage.getItem(STORAGE_KEY))
  if (saved) return prune(dedupeByFilm(saved), settings)
  // 首次进入：写入演示数据（页面会标注「本地演示数据」）
  const seeded = prune(DEMO_HISTORY, settings)
  saveHistory(seeded)
  return seeded
}

export function saveHistory(items: HistoryItem[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items))
  } catch {
    /* 存储不可用时忽略 */
  }
}

/** 写入/更新记录：每个「站点 + 影片」只保留一行，新记录覆盖旧行 */
export function upsertHistory(item: HistoryItem): HistoryItem[] {
  const settings = loadHistorySettings()
  const others = loadHistory().filter(
    (it) => !(it.siteKey === item.siteKey && it.id === item.id),
  )
  const next = prune([item, ...others], settings)
  saveHistory(next)
  return next
}

/** 删除记录：按「站点 + 影片」删除（epIndex 不参与） */
export function removeHistory(siteKey: string, id: string): HistoryItem[] {
  const next = loadHistory().filter((it) => !(it.siteKey === siteKey && it.id === id))
  saveHistory(next)
  return next
}

export function clearHistory(): HistoryItem[] {
  saveHistory([])
  return []
}
