// 观看历史状态（Pinia）：桌面端走 Rust SQLite 后端；浏览器预览走 mock + localStorage（页面标注「本地演示数据」）
// 后端语义：每个「站点 + 影片」只保留一行（新集覆盖旧行），列表按 updatedAt 倒序，写入后自动按保留策略裁剪。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { HistoryFilter, HistoryItem, HistorySettings } from '@/types'
import { isWithinWeek } from '@/utils/format'

/** 保留策略默认值（与后端一致：永久保存、最多 200 条） */
const DEFAULT_SETTINGS: HistorySettings = { retentionDays: 0, maxItems: 200 }

/**
 * 历史记录去重键：站点 + 影片 id（id 缺失时用片名兜底，与后端 `(site_key, name)` 兜底一致）。
 * 后端已保证 `(site_key, vod_id)` 唯一，这里是前端列表的二次兜底：
 * 合并旧缓存 / 并发写入 / 旧版本留下的重复数据，避免同一部片出现两张卡片。
 */
function historyKey(item: HistoryItem): string {
  const site = (item.siteKey || '').trim()
  const id = (item.id || '').trim()
  if (id) return `${site}\u0000${id}`
  return `${site}\u0000name:${(item.name || '').trim()}`
}

/** 合并同键记录：保留更新时间较新的元信息，同一集时进度取较大值。 */
function mergeItem(base: HistoryItem, next: HistoryItem): HistoryItem {
  const latest = next.updatedAt > base.updatedAt ? next : base
  const sameEpisode = base.epIndex === next.epIndex
  return {
    ...latest,
    positionSec: sameEpisode
      ? Math.max(base.positionSec || 0, next.positionSec || 0)
      : latest.positionSec || 0,
    durationSec: sameEpisode
      ? Math.max(base.durationSec || 0, next.durationSec || 0)
      : latest.durationSec || 0,
  }
}

/**
 * 列表规范化（兜底去重）：
 * 1. 按 `historyKey` 合并重复项（保留最新进度）；
 * 2. 若同一部片同时存在「有站点 key」与「空站点 key」两条记录，丢弃空站点那条
 *    （空站点记录点击后无法回到详情页，只会跳首页）；
 * 3. 最终按 updatedAt 倒序，并裁掉首尾空白的脏字段。
 */
function normalizeHistoryItems(items: HistoryItem[]): HistoryItem[] {
  const merged = new Map<string, HistoryItem>()
  for (const item of items) {
    if (!item) continue
    const clean: HistoryItem = {
      ...item,
      siteKey: (item.siteKey || '').trim(),
      siteName: (item.siteName || '').trim(),
      id: (item.id || '').trim(),
      name: (item.name || '').trim(),
      epTitle: (item.epTitle || '').trim(),
    }
    const key = historyKey(clean)
    const existing = merged.get(key)
    merged.set(key, existing ? mergeItem(existing, clean) : clean)
  }

  // 同一影片（id + 片名）若有一条可打开的记录，空站点副本直接丢弃。
  const openable = new Set<string>()
  for (const item of merged.values()) {
    if (item.siteKey) openable.add(`${item.id}\u0000${item.name}`)
  }
  return [...merged.values()]
    .filter((item) => Boolean(item.siteKey) || !openable.has(`${item.id}\u0000${item.name}`))
    .sort((a, b) => b.updatedAt - a.updatedAt)
}

export const useHistoryStore = defineStore('history', {
  state: () => ({
    items: [] as HistoryItem[],
    /** 历史记录保留策略（设置页「历史记录」分区读写） */
    settings: { ...DEFAULT_SETTINGS } as HistorySettings,
    filter: 'all' as HistoryFilter,
    loading: false,
    settingsLoading: false,
    initialized: false,
    error: '',
  }),

  getters: {
    /** 按筛选条件过滤后的列表（时间倒序） */
    filteredItems(state): HistoryItem[] {
      const list = [...state.items].sort((a, b) => b.updatedAt - a.updatedAt)
      if (state.filter === 'week') return list.filter((it) => isWithinWeek(it.updatedAt))
      if (state.filter === 'earlier') return list.filter((it) => !isWithinWeek(it.updatedAt))
      return list
    },
    /** 继续观看（最多 6 条，可按需在点播页顶部复用） */
    continueItems(state): HistoryItem[] {
      return [...state.items].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 6)
    },
    /** 某影片最近一次观看记录 */
    continueItem(state) {
      return (siteKey: string, id: string): HistoryItem | undefined =>
        state.items.find((it) => it.siteKey === siteKey && it.id === id)
    },
    isEmpty(state): boolean {
      return state.items.length === 0
    },
    total(state): number {
      return state.items.length
    },
    /** 保留策略的中文摘要（历史页 / 设置页展示用） */
    retentionLabel(state): string {
      const days = state.settings.retentionDays > 0 ? `最近 ${state.settings.retentionDays} 天` : '永久'
      const count = state.settings.maxItems > 0 ? `最多 ${state.settings.maxItems} 条` : '不限条数'
      return `${days} · ${count}`
    },
  },

  actions: {
    async load() {
      if (this.loading) return
      this.loading = true
      this.error = ''
      try {
        this.items = normalizeHistoryItems(await api.historyList())
      } catch (e) {
        this.error = api.formatError(e)
      } finally {
        this.loading = false
        this.initialized = true
      }
    },

    /** 读取保留策略（列表加载后或进入设置页时调用） */
    async loadSettings() {
      if (this.settingsLoading) return
      this.settingsLoading = true
      try {
        this.settings = await api.historySettings()
      } catch (e) {
        this.error = api.formatError(e)
      } finally {
        this.settingsLoading = false
      }
    },

    setFilter(filter: HistoryFilter) {
      this.filter = filter
    },

    /**
     * 更新保留策略（保存后后端立即裁剪）：
     * 同步 settings，并重新拉取列表以反映被清理掉的记录。
     * 失败时抛给调用方（设置页负责 toast）。
     */
    async updateSettings(patch: Partial<HistorySettings>) {
      const merged = { ...this.settings, ...patch }
      this.settings = await api.historySetSettings(
        Math.max(0, Math.floor(merged.retentionDays || 0)),
        Math.max(0, Math.floor(merged.maxItems || 0)),
      )
      this.items = normalizeHistoryItems(await api.historyList())
    },

    /**
     * 记录观看进度（同一影片只保留最新一行，新集覆盖旧行）。
     *
     * 缺站点 key 的记录写进去也无法回跳详情页（点击只会跳首页），
     * 所以在这里直接忽略；后端也有同样的兜底与「并入已知站点」处理。
     * vod_id 缺失时仍会提交，由后端按 `(site_key, name)` 兜底去重。
     */
    async upsert(item: HistoryItem) {
      if (!(item.siteKey || '').trim()) {
        console.warn('[vbox] 忽略缺少站点标识的历史记录:', item.name || item.id)
        return
      }
      this.items = normalizeHistoryItems(await api.historyUpsert(item))
    },

    /** 删除某影片的历史记录（按站点 + 影片，epIndex 不参与） */
    async remove(siteKey: string, id: string) {
      this.items = normalizeHistoryItems(await api.historyRemove(siteKey, id))
    },

    async clear() {
      this.items = normalizeHistoryItems(await api.historyClear())
    },
  },
})
