// 全局 UI 状态：主题（配色 × 亮/暗/跟随系统）、侧栏折叠、全局提示 toast
// 主题 class 的取值完全对齐 src/styles/theme.scss（听墨桌面端主题移植）：
//   html.theme-<亮色主题>   /   html.dark.theme-<暗色主题>
import { defineStore } from 'pinia'
import type { ThemeHue, ThemeMode, ToastLevel, ToastMessage, UiPersist } from '@/types'

const STORAGE_KEY = 'vbox.ui'
/** 侧栏自动折叠阈值（窗口 < 960px 折叠为图标模式，frontend-design.md §2.3） */
export const SIDEBAR_COLLAPSE_WIDTH = 960

/** 7 组配色 × 亮/暗 = 14 套主题；class 名必须与 theme.scss 中的选择器完全一致 */
export const THEME_HUES: ThemeHue[] = [
  {
    key: 'coral-rose',
    label: '赤霞',
    light: 'coral-rose',
    dark: 'crimson',
    colors: ['#CF6A60', '#F8EFEF', '#D47A7A', '#1E1818'],
  },
  {
    key: 'autumn-field',
    label: '暖橙',
    light: 'autumn-field',
    dark: 'ember',
    colors: ['#D49A6A', '#F8F2EA', '#CE9A6A', '#1E1C18'],
  },
  {
    key: 'sandalwood',
    label: '金砂',
    light: 'sandalwood',
    dark: 'gold',
    colors: ['#B89A6A', '#F5F0E6', '#BAA67A', '#1C1C18'],
  },
  {
    key: 'ink-green',
    label: '翠微',
    light: 'ink-green',
    dark: 'forest',
    colors: ['#4A6640', '#F4F6EF', '#6A9A7A', '#161C18'],
  },
  {
    key: 'frost-moon',
    label: '霜月',
    light: 'frost-moon',
    dark: 'night-blue',
    colors: ['#5A7A8A', '#F8F9FA', '#5BC0BE', '#1A2332'],
  },
  {
    key: 'ocean-blue',
    label: '蔚蓝',
    light: 'ocean-blue',
    dark: 'sapphire',
    colors: ['#3A5A7A', '#EBF0F5', '#6A8AB0', '#161A24'],
  },
  {
    key: 'lavender-mist',
    label: '烟紫',
    light: 'lavender-mist',
    dark: 'twilight-purple',
    colors: ['#6A5A7A', '#F5F2F8', '#BBA9C9', '#1E1A2E'],
  },
]

export const DEFAULT_HUE = 'ink-green'

function readPersisted(): Partial<UiPersist> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const data = JSON.parse(raw) as Partial<UiPersist>
    return data && typeof data === 'object' ? data : {}
  } catch {
    return {}
  }
}

/** 默认亚克力：仅 Windows 桌面端默认开启（那里有原生 Acrylic）；
 *  浏览器预览 / macOS（暂无原生毛玻璃）默认关闭，避免透明外壳叠在桌面/白底上影响可读性。 */
function defaultAcrylic(): boolean {
  try {
    const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
    const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
    return isTauri && isWindows
  } catch {
    return false
  }
}

function systemPrefersDark(): boolean {
  try {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
  } catch {
    return false
  }
}

let seq = 0
let inited = false

export const useUiStore = defineStore('ui', {
  state: () => ({
    /** 主题模式：跟随系统 / 亮色 / 暗色 */
    mode: 'system' as ThemeMode,
    /** 当前配色（THEME_HUES 的 key） */
    hue: DEFAULT_HUE,
    /** 侧栏是否折叠为图标模式 */
    sidebarCollapsed: false,
    /** 侧边与标题栏亚克力：Windows 桌面端默认开启（见 defaultAcrylic） */
    acrylic: defaultAcrylic(),
    /** 全局背景亚克力：依赖 acrylic，默认关闭 */
    acrylicContent: false,
    /** 全局提示（最多同时 3 条，4s 自动消失） */
    notices: [] as ToastMessage[],
  }),

  getters: {
    /** 当前配色的元数据（含亮/暗主题 class 名） */
    hueDef(state): ThemeHue {
      return THEME_HUES.find((h) => h.key === state.hue) ?? THEME_HUES[3]
    },
    /** 实际解析出的明暗模式 */
    resolvedMode(state): 'light' | 'dark' {
      if (state.mode === 'light' || state.mode === 'dark') return state.mode
      return systemPrefersDark() ? 'dark' : 'light'
    },
    isDark(): boolean {
      return this.resolvedMode === 'dark'
    },
    /** 当前主题 class（便于调试/测试） */
    themeClass(): string {
      const def = this.hueDef
      return this.isDark ? `dark theme-${def.dark}` : `theme-${def.light}`
    },
    /** 当前模式的中文名 */
    modeLabel(state): string {
      return state.mode === 'system' ? '跟随系统' : state.mode === 'dark' ? '暗色' : '亮色'
    },
    latestNotice(state): ToastMessage | null {
      return state.notices.length ? state.notices[state.notices.length - 1] : null
    },
  },

  actions: {
    /** 应用启动时调用一次：读取持久化偏好、应用主题、监听系统主题与窗口尺寸 */
    init() {
      if (inited) return
      inited = true
      const saved = readPersisted()
      if (saved.mode === 'system' || saved.mode === 'light' || saved.mode === 'dark') {
        this.mode = saved.mode
      }
      if (saved.hue && THEME_HUES.some((h) => h.key === saved.hue)) {
        this.hue = saved.hue
      }
      this.sidebarCollapsed =
        typeof saved.sidebarCollapsed === 'boolean'
          ? saved.sidebarCollapsed
          : window.innerWidth < SIDEBAR_COLLAPSE_WIDTH
      // 亚克力：默认开启「侧边与标题栏」，全局背景默认关闭；
      // 老版本持久化数据没有这两个字段时按默认值迁移。
      this.acrylic = typeof saved.acrylic === 'boolean' ? saved.acrylic : defaultAcrylic()
      this.acrylicContent = this.acrylic && saved.acrylicContent === true
      this.applyTheme()

      // 跟随系统：仅在 mode === 'system' 时生效
      try {
        const media = window.matchMedia('(prefers-color-scheme: dark)')
        media.addEventListener('change', () => {
          if (this.mode === 'system') this.applyTheme()
        })
      } catch {
        /* 环境不支持 matchMedia 时忽略 */
      }

      // 窗口宽度变化 → 只自动折叠（< 960px 图标模式），不自动展开：
      // 展开 / 折叠由用户在标题栏最左侧手动控制，避免拉扯用户的显式选择
      window.addEventListener('resize', () => {
        if (window.innerWidth < SIDEBAR_COLLAPSE_WIDTH && !this.sidebarCollapsed) {
          this.sidebarCollapsed = true
        }
      })
    },

    /** 把当前主题写入 <html>，并清除 index.html 的首屏兜底背景色 */
    applyTheme() {
      const def = this.hueDef
      const dark = this.resolvedMode === 'dark'
      // 只维护 dark / theme-* 两类，保留 acrylic-on 等其它类：
      // 整串覆盖 className 会把毛玻璃标记冲掉，导致切换主题后亚克力外壳失效。
      const root = document.documentElement
      root.classList.toggle('dark', dark)
      for (const cls of Array.from(root.classList)) {
        if (cls.startsWith('theme-')) root.classList.remove(cls)
      }
      root.classList.add(`theme-${dark ? def.dark : def.light}`)
      root.style.backgroundColor = ''
      this.applyAcrylic()
      this.persist()
    },

    /** 把亚克力开关同步到 <html>（acrylic-on / acrylic-on-content） */
    applyAcrylic() {
      const root = document.documentElement
      if (this.acrylic) {
        root.classList.add('acrylic-on')
        root.classList.toggle('acrylic-on-content', this.acrylicContent)
      } else {
        root.classList.remove('acrylic-on')
        root.classList.remove('acrylic-on-content')
      }
    },

    /** 侧边与标题栏亚克力开关；关闭时联动关闭全局背景 */
    setAcrylic(on: boolean) {
      this.acrylic = on
      if (!on) this.acrylicContent = false
      this.applyAcrylic()
      this.persist()
    },

    /** 全局背景亚克力开关（依赖侧边与标题栏） */
    setAcrylicContent(on: boolean) {
      this.acrylicContent = this.acrylic && on
      this.applyAcrylic()
      this.persist()
    },

    /** 恢复默认外观：跟随系统 + 默认配色 + 亚克力默认（侧边开、全局背景关） */
    resetAppearance() {
      this.mode = 'system'
      this.hue = DEFAULT_HUE
      this.acrylic = defaultAcrylic()
      this.acrylicContent = false
      this.applyTheme()
    },

    persist() {
      try {
        const data: UiPersist = {
          mode: this.mode,
          hue: this.hue,
          sidebarCollapsed: this.sidebarCollapsed,
          acrylic: this.acrylic,
          acrylicContent: this.acrylicContent,
        }
        localStorage.setItem(STORAGE_KEY, JSON.stringify(data))
      } catch {
        /* 存储不可用时忽略 */
      }
    },

    setMode(mode: ThemeMode) {
      this.mode = mode
      this.applyTheme()
    },

    setHue(hue: string) {
      if (!THEME_HUES.some((h) => h.key === hue)) return
      this.hue = hue
      this.applyTheme()
    },

    /** 侧栏主题按钮：亮色 → 暗色 → 跟随系统 → 亮色 */
    cycleMode() {
      const order: ThemeMode[] = ['light', 'dark', 'system']
      const next = order[(order.indexOf(this.mode) + 1) % order.length]
      this.setMode(next)
    },

    setSidebarCollapsed(collapsed: boolean) {
      this.sidebarCollapsed = collapsed
      this.persist()
    },

    toggleSidebar() {
      this.setSidebarCollapsed(!this.sidebarCollapsed)
    },

    /** 弹出全局提示（默认 4s 自动消失，最多同时 3 条） */
    notify(text: string, level: ToastLevel = 'info') {
      if (!text) return -1
      const id = ++seq
      this.notices.push({ id, text, level })
      if (this.notices.length > 3) this.notices.shift()
      window.setTimeout(() => this.dismissNotice(id), 4000)
      return id
    },

    dismissNotice(id: number) {
      this.notices = this.notices.filter((n) => n.id !== id)
    },

    clearNotices() {
      this.notices = []
    },
  },
})
