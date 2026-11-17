/**
 * 侧栏一级菜单的“最后位置”记忆。
 *
 * 路由 meta.nav 把详情页归到点播、本地播放归到下载、设置页归到设置；
 * 每次导航后把 fullPath 记到当前窗口的 sessionStorage，点一级菜单时优先回到
 * 该菜单上次停留的页面（而不是固定回 /vod 等首页）。
 *
 * 用 sessionStorage 而不是 localStorage：语义上接近 KeepAlive，
 * 应用窗口关闭后清空；同一窗口内刷新仍会保留。
 */
import type { RouteLocationNormalized } from 'vue-router'

const STORAGE_PREFIX = 'vbox.nav.last.'

function storageKey(nav: string): string {
  return `${STORAGE_PREFIX}${nav}`
}

/** 导航完成后记录：当前页面属于哪个一级菜单，就更新哪个菜单的最后位置。 */
export function rememberNavRoute(route: RouteLocationNormalized): void {
  const nav = typeof route.meta.nav === 'string' ? route.meta.nav : ''
  if (!nav) return
  // 独立日志窗口不参与主窗口的菜单位置记忆。
  if (route.path === '/log-window') return

  try {
    sessionStorage.setItem(storageKey(nav), route.fullPath)
  } catch {
    // sessionStorage 不可用（隐私模式/配额）时静默降级为“总是回首页”
  }
}

/** 清除某个一级菜单的“最后位置”，下次点击回到默认入口。 */
export function forgetNavRoute(nav: string): void {
  try {
    sessionStorage.removeItem(storageKey(nav))
  } catch {
    // sessionStorage 不可用时无需处理
  }
}

/** 取某个一级菜单上次停留的 fullPath；没有记录时回退到默认入口。 */
export function navTarget(nav: string, fallback: string): string {
  try {
    const saved = sessionStorage.getItem(storageKey(nav))
    if (saved && saved.startsWith('/')) return saved
  } catch {
    // 忽略读取失败，使用默认入口
  }
  return fallback
}
