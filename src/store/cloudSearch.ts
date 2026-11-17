// 云盘聚合搜索状态（W14 §7.3）：关键词、筛选、排序、分页、结果缓存与加载/错误态。
// KeepAlive 从详情/其它页返回本页时，同一「关键词|筛选|排序|页码」在 TTL 内直接复用缓存，
// 不重复请求后端；后端另有 50 关键词 LRU 兜底。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { CloudSearchPage, CloudSearchProvider, CloudSortBy } from '@/api'

export const CLOUD_SEARCH_PAGE_SIZE = 20
/** 前端结果缓存 TTL 与容量：只保会话内的最近查询，过期/超量自动淘汰 */
const CACHE_TTL_MS = 5 * 60 * 1000
const CACHE_MAX = 20

interface CachedPage {
  page: CloudSearchPage
  at: number
}

function cacheKeyOf(
  keyword: string,
  provider: CloudSearchProvider | '',
  sort: CloudSortBy,
  page: number,
): string {
  return `${keyword}|${provider}|${sort}|${page}`
}

function emptyPage(): CloudSearchPage {
  return {
    items: [],
    total: 0,
    partial: false,
    elapsedMs: 0,
    degradedSources: [],
    deduped: 0,
    providerCounts: [],
  }
}

export const useCloudSearchStore = defineStore('cloudSearch', {
  state: () => ({
    keyword: '',
    provider: '' as CloudSearchProvider | '',
    sort: 'relevance' as CloudSortBy,
    page: 1,
    pageSize: CLOUD_SEARCH_PAGE_SIZE,
    result: null as CloudSearchPage | null,
    loading: false,
    error: '',
    cache: {} as Record<string, CachedPage>,
  }),
  getters: {
    pageCount(state): number {
      if (!state.result || state.result.total <= 0) return 1
      return Math.max(1, Math.ceil(state.result.total / state.pageSize))
    },
    /** 去重后、provider 筛选前的分网盘计数（后端 SearchPage.providerCounts）。 */
    countsByProvider(state): Map<string, number> {
      const map = new Map<string, number>()
      for (const row of state.result?.providerCounts ?? []) map.set(row.provider, row.count)
      return map
    },
    allCount(state): number {
      return (state.result?.providerCounts ?? []).reduce((sum, row) => sum + row.count, 0)
    },
    /** 搜索源不可用：空结果且存在降级/超时来源（内置源依赖 Telegram，可能需要代理） */
    sourceTrouble(state): boolean {
      const value = state.result
      if (!value || value.items.length) return false
      return value.partial || value.degradedSources.length > 0
    },
    cacheKey(state): string {
      return cacheKeyOf(state.keyword, state.provider, state.sort, state.page)
    },
  },
  actions: {
    /** 路由 keyword 变化时重置筛选/排序/分页；同一关键词保留结果（返回本页不重复请求） */
    initialize(keyword: string): void {
      if (this.keyword === keyword) return
      this.keyword = keyword
      this.provider = ''
      this.sort = 'relevance'
      this.page = 1
      this.result = null
      this.error = ''
    },
    setFilter(provider: CloudSearchProvider | ''): void {
      if (this.provider === provider) return
      this.provider = provider
      this.page = 1
      void this.run(true)
    },
    setSort(sort: CloudSortBy): void {
      if (this.sort === sort) return
      this.sort = sort
      this.page = 1
      void this.run(true)
    },
    goPage(target: number): void {
      const next = Math.min(Math.max(1, target), this.pageCount)
      if (next === this.page) return
      this.page = next
      void this.run(true)
    },
    pruneCache(now = Date.now()): void {
      for (const [key, entry] of Object.entries(this.cache)) {
        if (now - entry.at > CACHE_TTL_MS) delete this.cache[key]
      }
      const keys = Object.keys(this.cache)
      if (keys.length <= CACHE_MAX) return
      keys.sort((a, b) => this.cache[a].at - this.cache[b].at)
      for (const key of keys.slice(0, keys.length - CACHE_MAX)) delete this.cache[key]
    },
    async run(force = false): Promise<void> {
      if (!this.keyword) return
      const key = this.cacheKey
      this.pruneCache()
      if (!force) {
        const hit = this.cache[key]
        if (hit) {
          this.result = hit.page
          this.error = ''
          return
        }
      }
      this.loading = true
      this.error = ''
      try {
        if (api.demoMode) {
          this.result = emptyPage()
          return
        }
        const page = await api.cloudSearch({
          keyword: this.keyword,
          page: this.page,
          pageSize: this.pageSize,
          providers: this.provider ? [this.provider] : [],
          sort: this.sort,
        })
        this.result = page
        this.cache[key] = { page, at: Date.now() }
      } catch (err) {
        this.error = err instanceof Error ? err.message : String(err)
        this.result = null
      } finally {
        this.loading = false
      }
    },
  },
})