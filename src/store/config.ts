// 订阅源状态（Pinia）：订阅组、线路展开、当前启用线路、站点缓存、仓库下拉、源 tab 记忆
import { defineStore } from 'pinia'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import { classifySite, siteStats as computeSiteStats } from '@/utils/siteSupport'
import type {
  ConfigInspection,
  ConfigKind,
  ConfigLine,
  LoadedConfig,
  Parser,
  Site,
  SiteStats,
  SourceHealth,
  SourceTabItem,
  Subscription,
  TvConfig,
  WarehouseOption,
} from '@/types'

/** 每个一级菜单上次选中的站点（源 tab 记忆），持久化在 localStorage */
const SITE_MEMORY_KEY = 'vbox.sites'

/** 初始化订阅失败后的自动重试次数（不含首次） */
const INIT_MAX_RETRIES = 2

/** 简单 sleep（用于初始化重试退避） */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

/** 订阅链接必须是完整的 http(s) 地址（前端先挡一层，避免把任意字符串写进配置） */
function isHttpUrl(value: string): boolean {
  try {
    const parsed = new URL(value)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

function readSiteMemory(): Record<string, string> {
  try {
    const raw = localStorage.getItem(SITE_MEMORY_KEY)
    if (!raw) return {}
    const data = JSON.parse(raw)
    return data && typeof data === 'object' ? (data as Record<string, string>) : {}
  } catch {
    return {}
  }
}

export const useConfigStore = defineStore('config', {
  state: () => ({
    /** 用户添加的订阅（后端持久化在应用数据目录） */
    subscriptions: [] as Subscription[],
    /** 已展开订阅的线路结构（key = 订阅 url） */
    addressGroups: {} as Record<string, ConfigInspection>,
    /** 哪个订阅处于展开状态 */
    expanded: {} as Record<string, boolean>,
    /** 当前启用线路 URL（全局仅一条） */
    enabled: '',
    /** 当前线路的完整加载结果（含 sourceUrl / kind / lines / format / cacheHit） */
    loaded: null as LoadedConfig | null,
    /** 当前线路所属的订阅/合集入口 URL（多仓切换/重启恢复用） */
    sourceUrl: '',
    /** 当前线路的完整配置（sites / lives / parses / flags / ...） */
    config: null as TvConfig | null,
    /** 站点列表（来自当前启用线路的配置） */
    sites: [] as Site[],
    /** 已统计过的线路源能力（key = 线路 URL；只缓存切换过的线路，不额外发网络请求） */
    lineStats: {} as Record<string, SiteStats>,
    /** 订阅形态：单配置 / 线路合集（多仓） */
    kind: 'single' as ConfigKind,
    /** 当前线路的线路列表（单配置时只有一项） */
    lines: [] as ConfigLine[],
    /** 配置解包格式：plain / base64-marker / aes-cbc-2423 / aes-ecb-pk */
    format: '',
    /** 本次加载是否命中配置缓存 */
    cacheHit: false,
    /** 每个一级菜单上次选中的站点 */
    lastSiteByMenu: readSiteMemory(),
    /** 源健康度记录（站点/线路连续失败；后端 source_health.db） */
    health: [] as SourceHealth[],
    healthLoaded: false,
    /** 定时刷新间隔（分钟，0 = 关闭） */
    refreshMinutes: 0,
    /** 上次定时刷新时间（毫秒） */
    lastRefreshAt: 0,
    /** 定时刷新 timer id（0 = 未启动） */
    refreshTimer: 0,
    loading: false,
    initialized: false,
    error: '',
    /** 浏览器预览模式：所有内容为演示数据（页面须标注） */
    demoMode: api.demoMode,
    demoInApp: api.demoInApp,
  }),

  getters: {
    /** 解析器列表（详情页解析器下拉用） */
    parsers(state): Parser[] {
      return state.config?.parses ?? []
    },
    /** 当前激活线路 URL（loaded.activeLine 的便捷只读视图） */
    activeLine(state): string {
      return state.loaded?.activeLine || state.enabled
    },
    /** 当前启用线路的名称（多仓线路可能没有 name，回退到订阅展开结果） */
    activeLineName(state): string {
      const line = state.lines.find((item) => item.url === state.enabled)
      if (line?.name) return line.name
      for (const group of Object.values(state.addressGroups)) {
        const found = group.lines.find((item) => item.url === state.enabled)
        if (found?.name) return found.name
      }
      return ''
    },
    /** 本机可用站点（type0/1/4 + type3 js / 原生规则引擎 / Python；jar 除外） */
    playableSites(state): Site[] {
      return state.sites.filter((s) => classifySite(s).local)
    },
    /** 当前线路的源能力统计（本机 / jar / Python / 未适配） */
    siteStats(state): SiteStats {
      return computeSiteStats(state.config?.sites ?? [])
    },
    /** 某条线路已缓存的源能力统计（未切换过的线路返回 undefined） */
    lineStatsOf(state) {
      return (url: string): SiteStats | undefined => state.lineStats[url]
    },
    /** 站点排序：本机可用在前，Python 未就绪其次，jar 靠后，未适配最后；同级保持后端原顺序 */
    sortedSites(state): Site[] {
      return state.sites
        .map((site, index) => ({ site, index, cap: classifySite(site) }))
        .sort((a, b) => a.cap.rank - b.cap.rank || a.index - b.index)
        .map((item) => item.site)
    },
    /** 顶部源 tab 数据（含能力级别 / 禁用原因 / 能力范围提示） */
    sourceTabs(): SourceTabItem[] {
      return this.sortedSites.map((s) => {
        const cap = classifySite(s)
        return {
          key: s.key,
          name: s.name,
          type: s.type,
          supported: cap.local,
          level: cap.level,
          note: cap.local ? cap.note : undefined,
          reason: cap.local ? undefined : cap.reason,
        }
      })
    },
    siteByKey(state) {
      return (key: string): Site | undefined => state.sites.find((s) => s.key === key)
    },
    /** 站点是否连续失败达到禁用阈值（>=3） */
    isSiteDisabled(state) {
      return (key: string): boolean =>
        state.health.some((h) => h.kind === 'site' && h.key === key && h.disabled)
    },
    /** 线路是否连续失败达到禁用阈值 */
    isLineDisabled(state) {
      return (key: string): boolean =>
        state.health.some((h) => h.kind === 'line' && h.key === key && h.disabled)
    },
    /** 某条源健康记录（展示失败次数/错误用）；不存在返回 undefined */
    healthOf(state) {
      return (kind: 'site' | 'line', key: string): SourceHealth | undefined =>
        state.health.find((h) => h.kind === kind && h.key === key)
    },
    /** 某菜单的当前站点 key（记忆 → 第一个本机可用站点 → 第一个站点） */
    activeSiteKey(state) {
      return (menu: string): string => {
        const remembered = state.lastSiteByMenu[menu]
        if (
          remembered &&
          state.sites.some((s) => s.key === remembered && classifySite(s).local)
        ) {
          return remembered
        }
        const first = state.sites.find((s) => classifySite(s).local) ?? state.sites[0]
        return first?.key ?? ''
      }
    },
    /** 点播主列表使用的站点 */
    activeSite(): Site | undefined {
      return this.siteByKey(this.activeSiteKey('vod'))
    },
    /** 仓库下拉：当前线路是线路合集（多仓）时返回 loaded.lines，并标记当前线路。
     *  后端 restore_enabled / set_enabled(sourceUrl, lineUrl) 会保持 kind=multi 且返回完整 lines；
     *  addressGroups 仅作为旧数据/异常时的兜底。 */
    warehouseOptions(state): WarehouseOption[] {
      let lines: ConfigLine[] = state.loaded?.kind === 'multi' ? state.loaded.lines : []
      if (!lines.length) {
        for (const group of Object.values(state.addressGroups)) {
          if (group.kind === 'multi' && group.lines.some((line) => line.url === state.enabled)) {
            lines = group.lines
            break
          }
        }
      }
      if (!lines.length) return []
      return lines.map((line) => ({
        name: line.name || line.url,
        url: line.url,
        active: line.url === state.enabled,
      }))
    },
    activeWarehouseName(): string {
      return this.warehouseOptions.find((o) => o.active)?.name ?? ''
    },
    /** 当前启用线路所属的订阅名（设置页展示用） */
    enabledSubscriptionName(state): string {
      for (const sub of state.subscriptions) {
        if (sub.url === state.enabled) return sub.name
        if (state.sourceUrl === sub.url) return sub.name
        if (state.loaded?.input === sub.url) return sub.name
        if (state.addressGroups[sub.url]?.lines.some((line) => line.url === state.enabled)) {
          return sub.name
        }
      }
      return ''
    },
  },

  actions: {
    /** 应用启动：读订阅列表 + 启用地址 + 启用配置。
     *  第三方订阅源经常抖动（连接被重置 / 响应体截断），这里做有限次自动重试，
     *  避免一次网络抖动就让整个点播页进入「配置读取失败」而用户不知道怎么办。 */
    async init(attempt = 0): Promise<void> {
      if (this.loading) return
      if (this.initialized && !this.error) return
      this.loading = true
      this.error = ''
      const ui = useUiStore()
      try {
        this.subscriptions = await api.getSubscriptions()
        const enabled = (await api.getEnabled()) || ''
        if (enabled) {
          // 重启恢复：restore_enabled 会带上持久化的 source + 线路，保持 kind=multi；
          // 旧数据/后端未加载时回退到按 enabled 直接加载（优先命中缓存）。
          let loaded: LoadedConfig | null = null
          try {
            loaded = await api.restoreEnabled()
          } catch {
            loaded = null
          }
          if (!loaded) loaded = await api.setEnabled(enabled)
          this.applyLoaded(loaded)
        } else {
          this.applyLoaded(null)
        }
        this.initialized = true
        void this.ensureAddressGroups()
        void this.loadHealth()
        try {
          this.refreshMinutes = (await api.subscriptionPrefs()).refreshMinutes
        } catch {
          this.refreshMinutes = 0
        }
        this.startAutoRefresh()
      } catch (e) {
        this.error = api.formatError(e)
        if (attempt < INIT_MAX_RETRIES) {
          // 自动重试：0.6s / 1.2s 退避后重新拉取
          this.loading = false
          await sleep(600 * (attempt + 1))
          return this.init(attempt + 1)
        }
        this.initialized = true
        ui.notify('订阅源加载失败：' + this.error + '（可在页面点「重试」）', 'error')
      } finally {
        this.loading = false
      }
    },

    /** 用户手动重试初始化（点播页错误态的「重试」按钮） */
    async retryInit() {
      if (this.loading) return
      this.initialized = false
      this.error = ''
      await this.init()
    },

    /** 应用后端 LoadedConfig：config/sites/enabled/sourceUrl/kind/lines/format/cacheHit 一起更新 */
    applyLoaded(loaded: LoadedConfig | null) {
      this.loaded = loaded
      this.config = loaded?.config ?? null
      this.sites = loaded?.config.sites ?? []
      this.enabled = loaded?.activeLine ?? ''
      this.sourceUrl = loaded?.sourceUrl ?? ''
      this.kind = loaded?.kind ?? 'single'
      this.lines = loaded?.lines ?? []
      this.format = loaded?.format ?? ''
      this.cacheHit = loaded?.cacheHit ?? false
      // 按线路缓存源能力统计（设置页线路列表展示；不额外请求后端）
      if (loaded) {
        this.lineStats = {
          ...this.lineStats,
          [loaded.activeLine]: computeSiteStats(loaded.config.sites ?? []),
        }
      }
    },

    /** 拉取所有订阅的线路列表（用于仓库下拉 / 设置页展开状态恢复）。
     *  串行请求，避免应用启动时对同一源站并发压太多请求；单个失败不阻断其它订阅。 */
    async ensureAddressGroups() {
      for (const sub of this.subscriptions) {
        try {
          this.addressGroups[sub.url] = await api.getAddresses(sub.name, sub.url)
        } catch {
          /* 单个订阅地址读取失败不影响其它订阅 */
        }
      }
    },

    rememberSite(menu: string, key: string) {
      this.lastSiteByMenu = { ...this.lastSiteByMenu, [menu]: key }
      try {
        localStorage.setItem(SITE_MEMORY_KEY, JSON.stringify(this.lastSiteByMenu))
      } catch {
        /* 忽略 */
      }
    },

    setActiveSite(menu: string, key: string) {
      this.rememberSite(menu, key)
    },

    /** 新增订阅（名称与链接均必填） */
    async addSubscription(name: string, url: string) {
      const n = name.trim()
      const u = url.trim()
      const ui = useUiStore()
      if (!n || !u) {
        ui.notify('订阅名称和链接均为必填', 'warning')
        return
      }
      if (!isHttpUrl(u)) {
        ui.notify('订阅链接需要是完整的 http(s) 地址', 'warning')
        return
      }
      try {
        const res = await api.addSubscription(n, u)
        this.subscriptions = res.subscriptions
        if (res.loaded) {
          // 首个订阅会自动启用，后端直接返回 loaded
          this.applyLoaded(res.loaded)
        } else if (res.enabled) {
          // 已有启用线路：仅在本地还没有加载结果时补一次（优先 restore 保持多仓上下文）
          this.enabled = res.enabled
          if (!this.loaded) {
            let loaded: LoadedConfig | null = null
            try {
              loaded = await api.restoreEnabled()
            } catch {
              loaded = null
            }
            this.applyLoaded(loaded ?? (await api.setEnabled(res.enabled)))
          }
        } else {
          this.applyLoaded(null)
        }
        void this.ensureAddressGroups()
        ui.notify('已添加订阅', 'success')
      } catch (e) {
        ui.notify('添加失败：' + api.formatError(e), 'error')
      }
    },

    /** 删除订阅，并刷新启用状态（先从本地列表移除，避免等待网络导致“像没删掉”） */
    async removeSubscription(url: string) {
      const ui = useUiStore()
      const index = this.subscriptions.findIndex((sub) => sub.url === url)
      const removed = index >= 0 ? this.subscriptions[index] : null
      if (index >= 0) {
        this.subscriptions = this.subscriptions.filter((sub) => sub.url !== url)
      }
      delete this.addressGroups[url]
      delete this.expanded[url]
      try {
        this.subscriptions = await api.removeSubscription(url)
        const enabled = (await api.getEnabled()) || ''
        if (enabled) {
          let loaded: LoadedConfig | null = null
          try {
            loaded = await api.restoreEnabled()
          } catch {
            loaded = null
          }
          this.applyLoaded(loaded ?? (await api.setEnabled(enabled)))
        } else {
          this.applyLoaded(null)
        }
        ui.notify('已删除订阅', 'success')
      } catch (e) {
        if (removed) {
          const restored = [...this.subscriptions]
          restored.splice(Math.max(0, Math.min(index, restored.length)), 0, removed)
          this.subscriptions = restored
        }
        ui.notify('删除失败：' + api.formatError(e), 'error')
      }
    },

    /** 展开/收起订阅线路 */
    async toggleExpand(name: string, url: string) {
      if (this.expanded[url]) {
        this.expanded[url] = false
        return
      }
      const ui = useUiStore()
      try {
        this.addressGroups[url] = await api.getAddresses(name, url)
        this.expanded[url] = true
      } catch (e) {
        ui.notify('读取地址失败：' + api.formatError(e), 'error')
      }
    },

    /** 启用某条线路（单仓 / 多仓子线路通用）。
     *  sourceUrl 传入该线路所属的订阅/合集入口，后端据此保持 kind=multi 并激活指定线路。 */
    async setEnabled(url: string, sourceUrl?: string) {
      const ui = useUiStore()
      const source = sourceUrl || this.sourceUrl || this.loaded?.sourceUrl || undefined
      try {
        this.applyLoaded(await api.setEnabled(url, false, source))
        ui.notify('已切换启用源', 'success')
        void this.loadHealth()
        return
      } catch (e) {
        // 线路加载失败：多仓时自动尝试下一条未禁用的线路（每条线路一次机会）
        const candidates = this.warehouseOptions.filter(
          (option) => option.url !== url && !this.isLineDisabled(option.url),
        )
        for (const line of candidates) {
          try {
            this.applyLoaded(await api.setEnabled(line.url, false, source))
            ui.notify(`线路加载失败，已自动切换到「${line.name}」`, 'warning')
            void this.loadHealth()
            return
          } catch {
            // 继续尝试下一条
          }
        }
        ui.notify('启用失败：' + api.formatError(e), 'error')
        void this.loadHealth()
      }
    },

    /** 编辑订阅名称/链接；若改的是当前启用地址，重新加载启用源 */
    async updateSubscription(oldUrl: string, name: string, url: string) {
      const ui = useUiStore()
      const n = name.trim()
      const u = url.trim()
      if (!n || !isHttpUrl(u)) {
        ui.notify('订阅链接需要是完整的 http(s) 地址', 'warning')
        throw new Error('invalid subscription url')
      }
      try {
        this.subscriptions = await api.updateSubscription(oldUrl, n, u)
        if (this.enabled === oldUrl) {
          try {
            this.applyLoaded(await api.reloadEnabled(true))
          } catch {
            // 重载失败由源健康度记录，不阻断保存
          }
        }
        ui.notify('订阅已更新', 'success')
      } catch (e) {
        ui.notify('保存订阅失败：' + api.formatError(e), 'error')
        throw e
      }
    },

    /** 从 JSON 文件导入订阅（按 URL 去重） */
    async importSubscriptionsFromFile() {
      const ui = useUiStore()
      try {
        const result = await api.importSubscriptions()
        if (!result) return
        this.subscriptions = result.subscriptions
        const skipped = result.skipped ? `，跳过 ${result.skipped} 个重复/无效条目` : ''
        ui.notify(`已导入 ${result.imported} 个订阅${skipped}`, 'success')
      } catch (e) {
        ui.notify('导入订阅失败：' + api.formatError(e), 'error')
      }
    },

    /** 导出订阅到 JSON 文件 */
    async exportSubscriptionsToFile() {
      const ui = useUiStore()
      try {
        const path = await api.exportSubscriptions()
        if (path) ui.notify(`订阅已导出：${path}`, 'success')
      } catch (e) {
        ui.notify('导出订阅失败：' + api.formatError(e), 'error')
      }
    },

    /** 设置定时刷新间隔并重启 timer */
    async setRefreshMinutes(minutes: number) {
      const ui = useUiStore()
      try {
        this.refreshMinutes = (await api.setSubscriptionRefresh(minutes)).refreshMinutes
        this.startAutoRefresh()
        ui.notify(
          this.refreshMinutes > 0 ? `已开启定时刷新（每 ${this.refreshMinutes} 分钟）` : '已关闭定时刷新',
          'success',
        )
      } catch (e) {
        ui.notify('保存定时刷新设置失败：' + api.formatError(e), 'error')
      }
    },

    /** 定时刷新：每 30s 检查一次是否到期（到期后静默 reload，失败由源健康度记录） */
    startAutoRefresh() {
      this.stopAutoRefresh()
      this.lastRefreshAt = Date.now()
      this.refreshTimer = window.setInterval(() => {
        if (this.refreshMinutes <= 0 || !this.enabled) return
        if (Date.now() - this.lastRefreshAt < this.refreshMinutes * 60_000) return
        this.lastRefreshAt = Date.now()
        void this.reload(false)
      }, 30_000)
    },

    stopAutoRefresh() {
      if (this.refreshTimer) {
        window.clearInterval(this.refreshTimer)
        this.refreshTimer = 0
      }
    },

    /** 读取源健康度记录（失败累计/禁用状态，用于变灰与自动切源） */
    async loadHealth() {
      try {
        this.health = await api.sourceHealthList()
        this.healthLoaded = true
      } catch {
        // 源健康度读取失败不影响主流程
      }
    },

    /** 当前站点失败后，找一个未禁用的可用站点（找不到返回 null） */
    nextHealthySite(excludeKey: string): string | null {
      const candidate = this.sortedSites.find(
        (site) => site.key !== excludeKey && !this.isSiteDisabled(site.key),
      )
      return candidate?.key ?? null
    },

    /** 重新加载当前启用线路（默认强制刷新，忽略配置缓存） */
    async reload(forceRefresh = true) {
      const ui = useUiStore()
      try {
        this.applyLoaded(await api.reloadEnabled(forceRefresh))
        ui.notify(forceRefresh ? '已刷新启用源' : '已重新加载启用源', 'success')
      } catch (e) {
        ui.notify('重新加载失败：' + api.formatError(e), 'error')
      }
    },

    /** 兼容旧调用：把提示转交全局 toast */
    setNotice(msg: string) {
      useUiStore().notify(msg)
    },
  },
})
