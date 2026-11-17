// 搜索状态（Pinia）：全站点搜索 + 按站点分组结果 + 关键词/热搜 + 本地分页
//
// 设计要点：
// - **默认全站点搜索**：搜索页不再有「当前站点 / 全部站点」开关，一次检索所有可搜索站点；
// - 结果按站点分组（`groups`），左侧站点列表展示各站命中数，右侧只渲染当前选中站点；
// - 分页没有按钮：页面滚动到底部时 `loadMore` 追加一页（结果已在内存，纯前端分页）；
// - 单站刷新只重新请求该站点（后端 scope='current'），不打断其它站点结果；
// - 关键词历史与热搜都持久化在 localStorage。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { DoubanHome, SearchHit } from '@/types'

const HISTORY_KEY = 'vbox.search.history'
const HOT_CACHE_KEY = 'vbox.search.hot'
const MAX_HISTORY = 20
/** 单页卡片数（滚到底部自动追加下一页） */
export const SEARCH_PAGE_SIZE = 24
/** 每次向后端多取一页的量：前端一页 24 条，后端一次取 48 条，减少往返 */
const SEARCH_FETCH_SIZE = SEARCH_PAGE_SIZE * 2

function hitKey(hit: SearchHit): string {
  return `${hit.siteKey}:${hit.id}`
}

/** 参与搜索的站点（页面从 config store 传入，避免 store 依赖配置 store） */
export interface SearchSiteRef {
  key: string
  name: string
}

/** 单个站点的搜索结果 */
export interface SearchSiteGroup {
  key: string
  name: string
  hits: SearchHit[]
  /** 后端报告的总命中数（用于继续翻页） */
  total: number
  /** 下一页页码；0 = 已取完 */
  nextPage: number
  /** 该站点正在检索（全量搜索或单站刷新） */
  loading: boolean
  /** 该站点搜索失败时的错误（不影响其它站点） */
  error: string
}

/** 搜索选中项：全部站点聚合，或某个具体站点 */
export type SearchSelection = 'all' | string

/** 热搜兜底词（豆瓣接口不可用 / 首次加载时展示，避免右侧空白） */
const HOT_FALLBACK = [
  '庆余年',
  '繁花',
  '狂飙',
  '三体',
  '漫长的季节',
  '与凤行',
  '琅琊榜',
  '甄嬛传',
  '热辣滚烫',
  '消失的她',
  '孤注一掷',
  '奥本海默',
  '沙丘2',
  '周处除三害',
  '哪吒之魔童闹海',
  '流浪地球2',
  '封神第一部',
  '长安三万里',
  '我的阿勒泰',
  '白夜追凶',
]

function readKeywords(): string[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    if (!raw) return []
    const data = JSON.parse(raw)
    return Array.isArray(data) ? data.filter((k): k is string => typeof k === 'string') : []
  } catch {
    return []
  }
}

function writeKeywords(keywords: string[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(keywords))
  } catch {
    /* 忽略 */
  }
}

function readHotCache(): string[] {
  try {
    const raw = localStorage.getItem(HOT_CACHE_KEY)
    if (!raw) return []
    const data = JSON.parse(raw)
    return Array.isArray(data) ? data.filter((k): k is string => typeof k === 'string') : []
  } catch {
    return []
  }
}

function writeHotCache(keywords: string[]) {
  try {
    localStorage.setItem(HOT_CACHE_KEY, JSON.stringify(keywords))
  } catch {
    /* 忽略 */
  }
}

/** 从豆瓣首页数据里提取热搜词：轮播 + 热门/热映集合优先，去重后取前 20 个 */
function extractHotKeywords(home: DoubanHome): string[] {
  const names: string[] = []
  const push = (title?: string | null) => {
    const value = title?.trim()
    if (value && !names.includes(value) && names.length < 20) names.push(value)
  }
  for (const item of home.carousel ?? []) push(item.title)
  const preferred = (home.collections ?? []).filter((c) => /热门|热映|TOP/i.test(c.name))
  const rest = (home.collections ?? []).filter((c) => !preferred.includes(c))
  for (const collection of [...preferred, ...rest]) {
    for (const item of collection.items ?? []) {
      if (names.length >= 20) break
      push(item.title)
    }
  }
  return names
}

export const useSearchStore = defineStore('search', {
  state: () => ({
    /** 当前输入框内容 */
    keyword: '',
    /** 本次实际提交的关键词（输入框之后可能被编辑） */
    lastKeyword: '',
    /** 全量搜索进行中（仍有站点在检索；结果会边到边展示） */
    loading: false,
    /** 搜索批次号：新搜索 / 清空时递增，旧批次的异步结果直接丢弃 */
    searchSeq: 0,
    /** 正在向后端请求下一页（防止滚动事件重复触发） */
    loadingMore: false,
    /** 全量搜索失败（单站失败不写这里，记录在 group.error） */
    error: '',
    /** 是否已经发起过搜索（区分初始态与空结果态） */
    searched: false,
    /** 按站点分组的结果（顺序 = 页面传入的站点顺序） */
    groups: [] as SearchSiteGroup[],
    /** 「全部」聚合展示顺序：按站点完成顺序追加，避免结果插队导致卡片跳动 */
    arrived: [] as string[],
    /** 当前选中：'all' 或站点 key */
    selectedKey: 'all' as SearchSelection,
    /** 每个分组已展示的条数（本地分页，'all' 单独一份） */
    visible: {} as Record<string, number>,
    /** 关键词历史（最多 20 条） */
    history: readKeywords() as string[],
    /** 热搜词（豆瓣首页派生；接口不可用时用兜底词） */
    hotKeywords: readHotCache().length ? readHotCache() : [...HOT_FALLBACK],
    hotLoading: false,
  }),

  getters: {
    /** 所有站点命中总数 */
    totalCount(state): number {
      return state.groups.reduce((sum, group) => sum + Math.max(group.total, group.hits.length), 0)
    },
    /** 有命中的站点数量 */
    hitSiteCount(state): number {
      return state.groups.filter((group) => group.hits.length > 0).length
    },
    /** 当前选中分组（选中 'all' 时为 undefined） */
    selectedGroup(state): SearchSiteGroup | undefined {
      if (state.selectedKey === 'all') return undefined
      return state.groups.find((group) => group.key === state.selectedKey)
    },
    /** 「全部」聚合用：有结果的站点按完成顺序排在前面（新结果只追加，不重排已有卡片） */
    orderedGroups(state): SearchSiteGroup[] {
      if (!state.arrived.length) return state.groups
      const order = new Map(state.arrived.map((key, index) => [key, index]))
      return [...state.groups].sort((a, b) => {
        const ai = order.get(a.key) ?? Number.MAX_SAFE_INTEGER
        const bi = order.get(b.key) ?? Number.MAX_SAFE_INTEGER
        return ai - bi
      })
    },
    /** 当前选中项的结果总数 */
    selectedTotal(state): number {
      if (state.selectedKey === 'all') {
        return state.groups.reduce((sum, group) => sum + group.hits.length, 0)
      }
      const group = state.groups.find((item) => item.key === state.selectedKey)
      return group?.hits.length ?? 0
    },
    /** 当前选中项已展示的条数 */
    visibleCount(state): number {
      return state.visible[state.selectedKey] ?? SEARCH_PAGE_SIZE
    },
    /** 是否还有下一页（已加载数据或尚未展示的数据均可继续） */
    hasMore(state): boolean {
      if (state.loadingMore) return false
      const target = state.selectedKey
      const groups =
        target === 'all' ? state.groups : state.groups.filter((group) => group.key === target)
      const total = groups.reduce((sum, group) => sum + group.total, 0)
      const loaded = groups.reduce((sum, group) => sum + group.hits.length, 0)
      const shown = state.visible[target] ?? SEARCH_PAGE_SIZE
      return shown < total || loaded < total
    },
  },

  actions: {
    setKeyword(keyword: string) {
      this.keyword = keyword
    },

    /** 选中站点（'all' = 聚合全部站点） */
    selectSite(key: SearchSelection) {
      this.selectedKey = key
      if (this.visible[key] === undefined) this.visible[key] = SEARCH_PAGE_SIZE
    },

    /** 滚到底部：先补齐后端未加载的页，再增加展示条数 */
    async loadMore(key?: SearchSelection) {
      if (this.loadingMore) return
      const target = key ?? this.selectedKey
      const seq = this.searchSeq
      const targetGroups =
        target === 'all' ? this.groups : this.groups.filter((group) => group.key === target)
      const pending = targetGroups.filter((group) => group.nextPage > 0 && group.hits.length < group.total)

      if (pending.length) {
        this.loadingMore = true
        try {
          await Promise.all(
            pending.map(async (group) => {
              const res = await api.searchPageVideos(this.lastKeyword, {
                scope: 'current',
                site: group.key,
                page: group.nextPage,
                pageSize: SEARCH_FETCH_SIZE,
              })
              if (seq !== this.searchSeq) return
              const known = new Set(group.hits.map(hitKey))
              for (const hit of res.items) {
                const id = hitKey(hit)
                if (!known.has(id)) {
                  known.add(id)
                  group.hits.push(hit)
                }
              }
              group.total = res.total
              group.nextPage = res.hasMore ? res.page + 1 : 0
            }),
          )
        } catch (e) {
          if (seq === this.searchSeq) this.error = api.formatError(e)
        } finally {
          if (seq === this.searchSeq) this.loadingMore = false
        }
      }
      if (seq !== this.searchSeq) return
      const total = targetGroups.reduce((sum, group) => sum + Math.max(group.total, group.hits.length), 0)
      const current = this.visible[target] ?? SEARCH_PAGE_SIZE
      if (current < total) this.visible[target] = Math.min(total, current + SEARCH_PAGE_SIZE)
    },

    /**
     * 全站点搜索：**按站点并发请求，完成一个推送一个**。
     *
     * 不再等后端把所有站点都搜完再一次性返回：先建立全部站点分组（loading），
     * 再用「搜索线程」设置作为并发数，每个站点返回后立即写入自己的分组，
     * 页面就能边搜边展示（左侧计数、右侧卡片同步增长）。
     */
    async searchAll(sites: SearchSiteRef[]) {
      const keyword = this.keyword.trim()
      if (!keyword) {
        this.resetResults()
        return
      }
      const seq = ++this.searchSeq
      this.lastKeyword = keyword
      this.searched = true
      this.error = ''
      this.loading = true
      this.loadingMore = false
      this.visible = { all: SEARCH_PAGE_SIZE }
      // 新关键词从「全部站点」开始（单站刷新/从详情返回时不会走到这里，选中项会保留）
      this.selectedKey = 'all'
      this.arrived = []
      this.groups = sites.map((site) => ({
        key: site.key,
        name: site.name,
        hits: [],
        total: 0,
        nextPage: 0,
        loading: true,
        error: '',
      }))
      if (!sites.length) {
        this.error = '当前线路没有可搜索的站点'
        this.loading = false
        return
      }
      this.remember(keyword)

      // 并发数取设置页的「搜索线程」；读取失败回退 8
      let threads = 8
      try {
        threads = (await api.searchPrefs()).threads
      } catch {
        /* 用默认线程数 */
      }
      if (seq !== this.searchSeq) return
      const concurrency = Math.min(Math.max(1, threads), sites.length)
      let cursor = 0

      const worker = async () => {
        while (true) {
          if (seq !== this.searchSeq) return
          const index = cursor
          cursor += 1
          if (index >= sites.length) return
          const site = sites[index]
          const group = this.groups.find((item) => item.key === site.key)
          if (!group) continue
          try {
            const res = await api.searchPageVideos(keyword, {
              scope: 'current',
              site: site.key,
              page: 1,
              pageSize: SEARCH_FETCH_SIZE,
            })
            if (seq !== this.searchSeq) return
            group.hits = res.items
            group.total = res.total
            group.nextPage = res.hasMore ? res.page + 1 : 0
            if (res.items.length && !this.arrived.includes(site.key)) this.arrived.push(site.key)
          } catch (e) {
            if (seq !== this.searchSeq) return
            group.hits = []
            group.total = 0
            group.nextPage = 0
            group.error = api.formatError(e)
          } finally {
            if (seq === this.searchSeq) group.loading = false
          }
        }
      }

      await Promise.all(Array.from({ length: concurrency }, () => worker()))
      if (seq === this.searchSeq) this.loading = false
    },

    /**
     * 刷新当前选中的结果：
     * - 选中「全部」→ 重新做一次全站点搜索；
     * - 选中某站点 → 只重搜该站点并替换该分组（其它站点结果保留）。
     */
    async refreshCurrent(sites: SearchSiteRef[]) {
      const keyword = this.keyword.trim()
      if (!keyword) return
      if (this.selectedKey === 'all') {
        await this.searchAll(sites)
        return
      }
      const key = this.selectedKey
      const group = this.groups.find((item) => item.key === key)
      if (!group) return
      group.loading = true
      group.error = ''
      try {
        const res = await api.searchPageVideos(keyword, {
          scope: 'current',
          site: key,
          page: 1,
          pageSize: SEARCH_FETCH_SIZE,
        })
        group.hits = res.items
        group.total = res.total
        group.nextPage = res.hasMore ? res.page + 1 : 0
        this.visible[key] = SEARCH_PAGE_SIZE
      } catch (e) {
        group.hits = []
        group.total = 0
        group.nextPage = 0
        group.error = api.formatError(e)
      } finally {
        group.loading = false
      }
    },

    remember(keyword: string) {
      const kw = keyword.trim()
      if (!kw) return
      this.history = [kw, ...this.history.filter((k) => k !== kw)].slice(0, MAX_HISTORY)
      writeKeywords(this.history)
    },

    removeKeyword(keyword: string) {
      this.history = this.history.filter((k) => k !== keyword)
      writeKeywords(this.history)
    },

    clearHistory() {
      this.history = []
      writeKeywords([])
    },

    /**
     * 加载热搜词：优先豆瓣首页（热门电影/剧集/综艺/动漫），失败时保留兜底词。
     * 缓存到 localStorage，下次进入搜索页先显示上次结果，再静默更新。
     */
    async loadHotKeywords() {
      if (this.hotLoading) return
      this.hotLoading = true
      try {
        const home = await api.doubanHome()
        const keywords = extractHotKeywords(home)
        if (keywords.length) {
          this.hotKeywords = keywords
          writeHotCache(keywords)
        }
      } catch {
        /* 网络不可用：保留兜底词 / 上次缓存 */
      } finally {
        this.hotLoading = false
      }
    },

    /** 清空当前搜索结果（保留关键词历史与热搜） */
    resetResults() {
      // 递增批次号：让仍在途的站点结果失效，不再写回已清空的分组
      this.searchSeq += 1
      this.lastKeyword = ''
      this.loading = false
      this.loadingMore = false
      this.error = ''
      this.searched = false
      this.groups = []
      this.arrived = []
      this.selectedKey = 'all'
      this.visible = {}
    },

    /** 清空输入与结果（Esc / 清空按钮） */
    reset() {
      this.keyword = ''
      this.resetResults()
    },
  },
})
