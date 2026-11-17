import type { EpisodeGroup } from '@/types'

/**
 * 线路 tab 的展示名 / 下载目录名。
 *
 * - 优先使用 group.group（如 hhyun、夸克网盘）；
 * - 为空时回退「线路N」；
 * - 同名线路第 2 个起追加 -2、-3，避免落到同一目录互相覆盖。
 *
 * 注意：Windows 路径大小写不敏感，重名判断用 lower-case。
 */
export function buildLineLabels(groups: EpisodeGroup[]): string[] {
  const counts = new Map<string, number>()
  return groups.map((group, index) => {
    const base = (group.group || '').trim() || `线路${index + 1}`
    const key = base.toLowerCase()
    const seen = (counts.get(key) ?? 0) + 1
    counts.set(key, seen)
    return seen === 1 ? base : `${base}-${seen}`
  })
}

/** 取第 index 条线路的展示名（越界时回退「线路N」） */
export function lineLabelAt(groups: EpisodeGroup[], index: number): string {
  const labels = buildLineLabels(groups)
  return labels[index] ?? `线路${index + 1}`
}
