/**
 * catvod 稳定错误码 → 面向用户的文案（标题 / 建议动作 / 是否可重试）。
 *
 * 与 `crates/vbox_core/src/spider/catvod/contract.rs` 的 `ErrorClass` 一一对应；
 * Rust 侧 `tests/catvod_error_copy.rs` 会读取本文件做一致性校验，改码时两边同步。
 */

export interface CatvodErrorCopy {
  /** 一句话标题（不含技术码） */
  title: string
  /** 建议动作 */
  action: string
  /** 是否值得原样重试 */
  retryable: boolean
}

export const CATVOD_ERROR_COPY: Record<string, CatvodErrorCopy> = {
  'catvod.jar.missing': {
    title: '缺少站点组件',
    action: '稍后重试，或在设置里更新订阅/镜像',
    retryable: true,
  },
  'catvod.jar.protected': {
    title: '站点组件受保护',
    action: '该站点无法在桌面端还原，建议在配置中禁用',
    retryable: false,
  },
  'catvod.spider.not_ported': {
    title: '该站点暂未支持',
    action: '关注后续版本更新；可先切换到其他站点',
    retryable: false,
  },
  'catvod.spider.script': {
    title: '脚本类站点',
    action: '检查脚本运行时是否可用，或换用其他站点',
    retryable: false,
  },
  'catvod.spider.unknown_api': {
    title: '无法识别的站点类型',
    action: '检查订阅里的 api 写法是否正确',
    retryable: false,
  },
  'catvod.jar.fetch': {
    title: '组件下载失败',
    action: '稍后重试，或在设置里更新订阅/镜像',
    retryable: true,
  },
  'catvod.jar.integrity': {
    title: '组件校验失败',
    action: '删除本地缓存后重新加载订阅',
    retryable: false,
  },
  'catvod.jar.layout': {
    title: '组件格式不支持',
    action: '该组件依赖 Android 布局，建议禁用该站点',
    retryable: false,
  },
  'catvod.result.parse': {
    title: '站点数据解析失败',
    action: '站点可能已改版，稍后重试或换线路/换站点',
    retryable: true,
  },
  'catvod.host.unsupported': {
    title: '需要桌面端不支持的能力',
    action: '换用其他站点；该能力需要 Android/WebView 宿主',
    retryable: false,
  },
  'catvod.net.http': {
    title: '网络请求失败',
    action: '检查网络/代理后重试，或换镜像',
    retryable: true,
  },
  'catvod.net.dns': {
    title: '域名解析失败',
    action: '检查 DNS 或代理设置后重试',
    retryable: true,
  },
  'catvod.net.connect': {
    title: '无法连接站点',
    action: '换镜像/代理后重试',
    retryable: true,
  },
  'catvod.net.timeout': {
    title: '请求超时',
    action: '稍后重试或换镜像',
    retryable: true,
  },
  'catvod.net.tls': {
    title: 'HTTPS 证书错误',
    action: '检查系统时间/证书，或换镜像',
    retryable: true,
  },
  'catvod.pan.share_unavailable': {
    title: '网盘分享已失效',
    action: '换一个分享链接/线路，或联系分享者更新',
    retryable: false,
  },
  'catvod.pan.cookie_required': {
    title: '网盘需要登录 Cookie',
    action: '在「云盘 → 右上角账号管理」登录，或在站点 ext 中配置对应网盘的 Cookie 后重试',
    retryable: false,
  },
  'catvod.live.fetch': {
    title: '直播源加载失败',
    action: '检查订阅地址/格式与网络后重试',
    retryable: true,
  },
  'catvod.play.parse': {
    title: '解析播放地址失败',
    action: '换解析器/换线路，或检查解析器配置',
    retryable: true,
  },
  'catvod.aux.fetch': {
    title: 'EPG/字幕加载失败',
    action: '检查地址与网络后重试',
    retryable: true,
  },
}

/** 常见码的展示顺序（长码优先，避免前缀命中） */
const CODES_BY_LENGTH = Object.keys(CATVOD_ERROR_COPY).sort((a, b) => b.length - a.length)

export function detectCatvodErrorCode(message: string): string | null {
  for (const code of CODES_BY_LENGTH) {
    if (message.includes(code)) return code
  }
  return null
}

/**
 * 把带 `catvod.*` 码的消息翻译成「标题：细节（建议动作）」。
 * 没有稳定码时返回 `null`，调用方保留原文。
 */
export function translateCatvodError(input: string): string | null {
  const code = detectCatvodErrorCode(input)
  if (!code) return null
  const copy = CATVOD_ERROR_COPY[code]
  if (!copy) return null

  const detail = input
    .split(code)
    .slice(1)
    .join(code)
    .replace(/^[:：\s\-—]+/, '')
    .trim()

  const suffix = copy.retryable ? `${copy.action}，可重试` : copy.action
  if (!detail) return `${copy.title}（${suffix}）`
  return `${copy.title}：${detail}（${suffix}）`
}
