// 通用格式化工具（时间 / 体积 / 速率），不涉及颜色与主题

/** 秒 → mm:ss（超过 1 小时为 h:mm:ss） */
export function formatDuration(totalSec: number): string {
  const sec = Math.max(0, Math.floor(totalSec || 0))
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = sec % 60
  const mm = String(m).padStart(2, '0')
  const ss = String(s).padStart(2, '0')
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`
}

/** 字节 → 人类可读体积 */
export function formatBytes(bytes?: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[i]}`
}

/** 字节/秒 → 速率文案 */
export function formatSpeed(bps?: number): string {
  if (!bps || bps <= 0) return '—'
  return `${formatBytes(bps)}/s`
}

/** 时间戳 → 相对时间（今天 / 昨天 / 更早日期） */
export function formatRelativeTime(ts: number): string {
  if (!ts) return ''
  const date = new Date(ts)
  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const diff = startOfToday - new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
  const hm = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
  if (diff === 0) return `今天 ${hm}`
  if (diff === 24 * 60 * 60 * 1000) return `昨天 ${hm}`
  if (date.getFullYear() === now.getFullYear()) return `${date.getMonth() + 1}月${date.getDate()}日 ${hm}`
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

/** 时间戳 → 最近的观看时间分组标签（本周 / 更早） */
export function isWithinWeek(ts: number): boolean {
  return Date.now() - ts <= 7 * 24 * 60 * 60 * 1000
}
