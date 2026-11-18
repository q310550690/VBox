// 源能力判定（本机支持 / 后续支持）统一收口
// - 本机支持：type0(XML) / type1(苹果CMS) / type4(HTTP 远程接口)
//            type3 `.js` / 原生规则引擎 `csp_XYQHiker*`、`csp_XBPQ*`
//            type3 Python `.py`/`py_`（CPython sidecar，需系统 Python）
// - 后续支持：type3 jar（需 Android Host）
// 页面（源 tab / 站点弹窗 / 设置页线路统计 / 搜索空态 / 错误态）都从这里取结论，
// 避免再出现“暂不支持”这类绝对措辞。
import { shallowRef } from 'vue'
import type { Site, SiteApiSupport, SiteStats, SiteSupportLevel } from '@/types'

/**
 * Python spider 能力开关。
 * 后端（crates/vbox_core/src/python，CPython sidecar）已接入：`.py`/`py_` 进入本机支持，
 * 但运行时依赖系统 Python（VBOX_PYTHON / python / python3 / py -3），因此标注“需系统 Python”。
 * 若后端回退到未接入状态，把此常量改回 false 即可自动回到“后续支持”文案。
 */
export const PYTHON_SPIDER_READY = true
/** drpy Node 运行时已接入（Rust node 模块 + host.cjs）；仍需用户安装 Node 18+ */
export const NODE_DRPY_READY = true

/** 后端原生 XYQHiker 引擎支持的 api 名（与 sources/spider.rs 对齐） */
const XYQ_APIS = ['csp_XYQHiker', 'csp_XYQHikerAL', 'csp_XYQHikerV2']
/** 后端原生 XBPQ 规则引擎（实验性）支持的 api 名 */
const XBPQ_APIS = ['csp_XBPQ', 'csp_XBPQ_upgraded']

export interface SiteCapability {
  level: SiteSupportLevel
  /** 本机是否可加载 */
  local: boolean
  /** 排序权重：越小越靠前（本机 0 / Python 未就绪 1 / jar 2 / 其它 3） */
  rank: number
  /** 本机可用时的能力说明 */
  note?: string
  /** 本机不可用时的原因（统一“后续支持 / 暂未适配”措辞） */
  reason?: string
}

function apiOf(site: Site): string {
  return site.api.trim()
}

/** type3 的 .js spider（与后端 api_is_js 判定一致） */
export function isJsSpider(site: Site): boolean {
  const api = apiOf(site)
  return api.endsWith('.js') || api.includes('.js?')
}

/** drpy/drpy2 框架脚本：依赖 cheerio/ES module，当前后端不能完整运行 */
export function isDrpySpider(site: Site): boolean {
  const api = apiOf(site).toLowerCase()
  if (api.includes('drpy')) return true
  const ext = site.ext
  if (typeof ext === 'string') {
    const lower = ext.toLowerCase()
    return lower.includes('drpy2.min.js') || lower.includes('/drpy.js') || lower.endsWith('drpy.js')
  }
  return false
}

/** type3 的 XYQHiker 原生规则引擎（不执行 jar） */
export function isXyqSpider(site: Site): boolean {
  return XYQ_APIS.includes(apiOf(site))
}

/** type3 的 XBPQ 原生规则引擎（实验性，不执行 jar） */
export function isXbpqSpider(site: Site): boolean {
  return XBPQ_APIS.includes(apiOf(site))
}

/** type3 的 Python spider（`.py` / `py_` 前缀 / inline Python / ext 里的脚本地址） */
export function isPythonSpider(site: Site): boolean {
  const api = apiOf(site)
  const lower = api.toLowerCase()
  if (lower.startsWith('inline:')) {
    // inline: 由 JS/Python 共用，用 Python 语法特征区分（与后端 inline_looks_python 对齐）
    return [
      'base.spider',
      'class Spider',
      'def homeContent',
      'def categoryContent',
      'import base',
    ].some((token) => api.includes(token))
  }
  if (lower.startsWith('py_') || lower.includes('.py')) return true
  return extLooksLikeScript(site)
}

/** ext 里是否藏着 Python 脚本地址（只认 `.py`，避免把普通 ext 误判成 Python） */
function extLooksLikeScript(site: Site): boolean {
  const looksLikePython = (value: string): boolean => {
    const text = value.trim().toLowerCase()
    return text.endsWith('.py') || text.includes('.py?')
  }
  const ext = site.ext
  if (typeof ext === 'string') return looksLikePython(ext)
  if (ext && typeof ext === 'object' && !Array.isArray(ext)) {
    const obj = ext as Record<string, unknown>
    for (const key of ['url', 'api', 'script', 'spider', 'path', 'py']) {
      const value = obj[key]
      if (typeof value === 'string' && looksLikePython(value)) return true
    }
  }
  if (ext == null) return false
  try {
    const text = typeof ext === 'string' ? ext : JSON.stringify(ext)
    return text.trim().toLowerCase().includes('.py')
  } catch {
    return false
  }
}

/** type3 的 jar spider：既不是 js / 原生规则引擎，也不是 Python */
export function isJarSpider(site: Site): boolean {
  if (site.type !== 3) return false
  return (
    !isJsSpider(site) &&
    !isDrpySpider(site) &&
    !isXyqSpider(site) &&
    !isXbpqSpider(site) &&
    !isPythonSpider(site)
  )
}

/**
 * REA 画像缓存（api → 支持画像）。
 *
 * 站点选择器/统计用到的 `classifySite` 是纯函数，这里用 shallowRef 保存画像：
 * 在 computed/模板中调用时会自动建立响应式依赖，画像异步加载完成后自动重算。
 */
const apiSupport = shallowRef<Record<string, SiteApiSupport>>({})

/** 写入 REA 画像（`site_api_support` 命令结果）；重复调用整体替换。 */
export function setSiteApiSupport(rows: SiteApiSupport[]): void {
  const map: Record<string, SiteApiSupport> = {}
  for (const row of rows) {
    if (row.api) map[row.api] = row
  }
  apiSupport.value = map
}

/** 当前 api 的画像（无画像返回 undefined）。 */
function supportOf(site: Site): SiteApiSupport | undefined {
  const api = apiOf(site)
  return api ? apiSupport.value[api] : undefined
}

/** 待移植/不支持原因：优先 REA 阻塞原因解释，其次画像 reason。 */
function blockedText(profile: SiteApiSupport | undefined, fallback: string): string {
  if (!profile) return fallback
  return profile.blockedHint || profile.reason || fallback
}

/** 判定单个源的本机支持能力与展示文案 */
export function classifySite(site: Site): SiteCapability {
  switch (site.type) {
    case 0:
      return { level: 'local', local: true, rank: 0, note: 'XML' }
    case 1:
      return { level: 'local', local: true, rank: 0 }
    case 4:
      return { level: 'local', local: true, rank: 0, note: 'HTTP 远程接口' }
    case 3: {
      if (isXyqSpider(site)) {
        return { level: 'local', local: true, rank: 0, note: 'XYQHiker 原生规则引擎' }
      }
      if (isXbpqSpider(site)) {
        return { level: 'local', local: true, rank: 0, note: '原生规则引擎（实验性）' }
      }
      if (isDrpySpider(site)) {
        return NODE_DRPY_READY
          ? {
              level: 'local',
              local: true,
              // 依赖 Node：排在纯本机源之后、Python/jar 之前
              rank: 1,
              note: 'drpy 运行时（需 Node.js）',
            }
          : {
              level: 'unsupported',
              local: false,
              rank: 3,
              reason: 'drpy 需要 Node.js 18+；请安装后在设置页指定 Node 路径',
            }
      }
      if (isJsSpider(site)) {
        return { level: 'local', local: true, rank: 0, note: 'js spider 子集' }
      }
      if (isPythonSpider(site)) {
        return PYTHON_SPIDER_READY
          ? {
              level: 'local',
              local: true,
              // 依赖系统 Python：排序放在纯本机源（XML/JS/规则引擎）之后、jar 之前
              rank: 1,
              note: 'Python 运行时（需系统 Python）',
            }
          : {
              level: 'python',
              local: false,
              rank: 1,
              reason: '后续支持（需系统 Python 运行时）',
            }
      }
      const profile = supportOf(site)
      // REA 画像已判定为原生实现（规则引擎 / 原生解析家族）：不再误标「待移植」。
      if (profile?.native || profile?.strategy === 'rulepack') {
        return { level: 'local', local: true, rank: 0, note: '原生实现（REA 画像）' }
      }
      if (profile?.strategy === 'unported') {
        return {
          level: 'jar',
          local: false,
          rank: 2,
          reason: blockedText(profile, '待移植（需 Android Host）'),
        }
      }
      if (profile?.strategy === 'unsupported') {
        return {
          level: 'unsupported',
          local: false,
          rank: 3,
          reason: blockedText(profile, 'REA 画像判定为不支持'),
        }
      }
      if (profile) {
        return {
          level: 'jar',
          local: false,
          rank: 2,
          reason: blockedText(profile, '待移植（需 Android Host）'),
        }
      }
      return {
        level: 'jar',
        local: false,
        rank: 2,
        reason: '待评估（未在 REA 画像中命中）',
      }
    }
    default:
      return {
        level: 'unsupported',
        local: false,
        rank: 3,
        reason: `暂未适配（type=${site.type}）`,
      }
  }
}

/** 统计一组源的本机能力（设置页线路标签 / 搜索空态用，不发起网络请求） */
export function siteStats(sites: Site[]): SiteStats {
  const stats: SiteStats = {
    total: sites.length,
    local: 0,
    jar: 0,
    python: 0,
    unsupported: 0,
  }
  for (const site of sites) {
    const { level } = classifySite(site)
    if (level === 'local') stats.local += 1
    else if (level === 'jar') stats.jar += 1
    else if (level === 'python') stats.python += 1
    else stats.unsupported += 1
  }
  return stats
}

/** 线路统计短文案：`本机 8 · jar 3 · Python 1` */
export function describeSiteStats(stats: SiteStats): string {
  const parts = [`本机 ${stats.local}`]
  if (stats.jar) parts.push(`jar ${stats.jar}`)
  if (stats.python) parts.push(`Python ${stats.python}`)
  if (stats.unsupported) parts.push(`未适配 ${stats.unsupported}`)
  return parts.join(' · ')
}
