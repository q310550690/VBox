// 收藏状态（Pinia）：桌面端走 Rust SQLite 后端；浏览器预览走 mock + localStorage
// 后端语义：按「站点 + 影片」唯一（同 key UPSERT），列表按 createdAt 倒序。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { FavoriteFilter, FavoriteItem } from '@/types'
import { isWithinWeek } from '@/utils/format'

export const useFavoritesStore = defineStore('favorites', {
  state: () => ({
    items: [] as FavoriteItem[],
    /** 收藏筛选：全部 / 本周 / 更早（按 createdAt） */
    filter: 'all' as FavoriteFilter,
    loading: false,
    initialized: false,
    error: '',
  }),

  getters: {
    /** 收藏总数 */
    total(state): number {
      return state.items.length
    },
    /** 按筛选条件过滤后的收藏（createdAt 倒序） */
    filteredItems(state): FavoriteItem[] {
      const list = [...state.items].sort((a, b) => b.createdAt - a.createdAt)
      if (state.filter === 'week') return list.filter((it) => isWithinWeek(it.createdAt))
      if (state.filter === 'earlier') return list.filter((it) => !isWithinWeek(it.createdAt))
      return list
    },
    /** 是否为空（空态判定用） */
    isEmpty(state): boolean {
      return state.items.length === 0
    },
    /** 某影片是否已收藏（详情页按钮状态用） */
    isFavorite(state) {
      return (siteKey: string, vodId: string): boolean =>
        state.items.some((it) => it.siteKey === siteKey && it.vodId === vodId)
    },
  },

  actions: {
    setFilter(filter: FavoriteFilter) {
      this.filter = filter
    },

    /** 拉取收藏列表（失败只记录错误，不抛，页面展示错误态） */
    async load() {
      if (this.loading) return
      this.loading = true
      this.error = ''
      try {
        this.items = await api.favoriteList()
      } catch (e) {
        this.error = api.formatError(e)
      } finally {
        this.loading = false
        this.initialized = true
      }
    },

    /** 新增 / 更新收藏（后端按 siteKey + vodId UPSERT），失败抛给调用方 */
    async add(item: FavoriteItem) {
      this.items = await api.favoriteAdd({
        ...item,
        createdAt: item.createdAt > 0 ? item.createdAt : Date.now(),
      })
      this.initialized = true
    },

    /** 取消收藏，失败抛给调用方 */
    async remove(siteKey: string, vodId: string) {
      this.items = await api.favoriteRemove(siteKey, vodId)
      this.initialized = true
    },

    /**
     * 切换收藏状态：已收藏则取消，否则新增。
     * @returns 操作后的收藏状态（true=已收藏）
     * 失败抛出，由调用方 toast。
     */
    async toggle(item: FavoriteItem): Promise<boolean> {
      const exists = this.isFavorite(item.siteKey, item.vodId)
      if (exists) {
        await this.remove(item.siteKey, item.vodId)
        return false
      }
      await this.add(item)
      return true
    },
  },
})
