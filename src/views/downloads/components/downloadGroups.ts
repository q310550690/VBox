// 下载任务 → 「一部剧一个卡片」的分组数据（纯函数，便于下载页与卡片组件复用/测试）
import type { DownloadStatus, DownloadTask } from '@/types'
import { formatSpeed } from '@/utils/format'

/** 分组后的剧集卡片数据 */
export interface DownloadSeriesGroup {
  /** 稳定分组键（站点 + 影片 id/名称 + 线路） */
  key: string
  name: string
  siteName?: string
  /** 线路 tab 名（下载目录第三层） */
  line?: string
  cover?: string
  /** 组内任务（按集序号/创建时间升序） */
  tasks: DownloadTask[]
  total: number
  /** 进行中（queued/downloading/paused）集数 */
  active: number
  downloading: number
  completed: number
  failed: number
  /** 总进度 0-100：所有任务体积已知时按字节加权，否则按每集进度等权估算 */
  progress: number
  /**
   * 是否有「总大小未知」的未完成任务（HLS 下载中 / 服务器没给 Content-Length）。
   * true 时进度是估算值，UI 应显示「不定进度 + 已下载大小」，且绝不能显示成 100%。
   */
  indeterminate: boolean
  /** 当前总速度（字节/秒，仅统计下载中的集） */
  speedBps: number
  /** 已知总字节数（未知的集不计入） */
  sizeBytes: number
  /** 已下载字节数（包含总大小未知的集） */
  downloadedBytes: number
  /** 组整体状态（用于卡片状态色） */
  status: DownloadStatus
  /** 最近一次任务变化时间（用于排序） */
  latestAt: number
}

/** 状态优先级：越靠前越「热闹」，用于推导整组状态 */
const STATUS_PRIORITY: DownloadStatus[] = [
  'downloading',
  'paused',
  'queued',
  'failed',
  'completed',
]

/** 合并同一集的重复下载记录（同一条线路重复点击会产生多条），避免卡片里出现重复集。
 *  身份：线路 + 集标题（epTitle），退化到线路 + 集序号（epIndex）；保留最「活跃」的一条。
 *  不同线路必须分开，否则同集的多线路版本会被误合并成一条。 */
const STATUS_RANK: Record<DownloadStatus, number> = {
  downloading: 5,
  queued: 4,
  paused: 3,
  failed: 2,
  completed: 1,
}

function mergeDuplicateEpisodes(tasks: DownloadTask[]): DownloadTask[] {
  const byEpisode = new Map<string, DownloadTask>()
  for (const task of tasks) {
    const title = (task.epTitle || '').trim()
    const line = (task.line || '').trim()
    const key = `${line}::${title || `#${task.epIndex}`}`
    const existing = byEpisode.get(key)
    if (!existing) {
      byEpisode.set(key, task)
      continue
    }
    const rankA = STATUS_RANK[existing.status] ?? 0
    const rankB = STATUS_RANK[task.status] ?? 0
    if (rankB > rankA) {
      byEpisode.set(key, task)
    } else if (rankB === rankA && (task.updatedAt ?? 0) >= (existing.updatedAt ?? 0)) {
      byEpisode.set(key, task)
    }
  }
  return [...byEpisode.values()]
}

const clampPercent = (value: number): number => {
  if (!Number.isFinite(value)) return 0
  return Math.max(0, Math.min(100, value))
}

/** 单集进度 0-100：以后端 `progress` 字段为准（HLS 按分片比例，直链按字节比例） */
export function taskPercent(task: DownloadTask): number {
  if (task.status === 'completed') return 100
  return clampPercent(task.progress)
}

/**
 * 单集「已知总字节数」；未知时返回 0。
 *
 * HLS 任务在下载完成前没有整集总大小（后端 `total_bytes` 保持 0，只用 `progress`
 * 表示分片比例）。老版本曾把 `total_bytes` 写成「已下载字节」（等于 downloaded），
 * 会被误算成 100%，这里一并按「未知」处理。
 */
export function taskTotalBytes(task: DownloadTask): number {
  const size = task.sizeBytes ?? 0
  if (size <= 0) return 0
  if (task.status !== 'completed' && task.kind === 'hls') {
    const downloaded = Math.max(0, task.downloadedBytes ?? 0)
    if (downloaded >= size) return 0
  }
  return size
}

/** 单集已下载字节；总大小未知时用后端统计的下载字节数 */
export function taskDownloadedBytes(task: DownloadTask): number {
  const total = taskTotalBytes(task)
  const downloaded = Math.max(0, task.downloadedBytes ?? 0)
  if (task.status === 'completed') {
    return total > 0 ? total : downloaded
  }
  if (total > 0) {
    if (downloaded > 0) return Math.min(total, downloaded)
    return (taskPercent(task) / 100) * total
  }
  return downloaded
}

/** 单集排序键：优先 epIndex（全局集序号），缺失时回退创建时间 */
function episodeOrder(a: DownloadTask, b: DownloadTask): number {
  const ai = typeof a.epIndex === 'number' ? a.epIndex : Number.POSITIVE_INFINITY
  const bi = typeof b.epIndex === 'number' ? b.epIndex : Number.POSITIVE_INFINITY
  if (ai !== bi) return ai - bi
  return (a.createdAt || 0) - (b.createdAt || 0)
}

/** 最近变化时间：优先 updatedAt，回退 createdAt */
function changedAt(task: DownloadTask): number {
  return task.updatedAt || task.createdAt || 0
}

/**
 * 把扁平的下载任务按「站点 + 影片 + 线路」聚合为剧集卡片：
 * - 同剧同线路的不同集归到同一组；同剧不同线路各自一张卡（目录不同）；
 * - 组内按集序号升序；
 * - 组间进行中优先，其次按最近变化时间倒序。
 *
 * 外层进度不再出现「100% 但子集 2%」：
 * - 所有任务体积已知 → Σ已下载 / Σ总大小；
 * - 有任务体积未知（HLS 下载中）→ 按每集进度等权估算，并标记 `indeterminate`。
 */
export function buildDownloadGroups(tasks: DownloadTask[]): DownloadSeriesGroup[] {
  const buckets = new Map<string, DownloadTask[]>()
  for (const task of tasks) {
    // 没有 vodId 的旧任务/演示数据：退化为「站点 + 片名」，避免同剧被拆散
    const filmKey = task.vodId || task.name || task.id
    const line = (task.line || '').trim()
    const key = `${task.siteKey || ''}::${filmKey}::${line}`
    const list = buckets.get(key)
    if (list) list.push(task)
    else buckets.set(key, [task])
  }

  const groups: DownloadSeriesGroup[] = []
  for (const [key, list] of buckets) {
    // 同集多次下载（同线路重复点击）只展示最近/最活跃的一条
    const sorted = mergeDuplicateEpisodes(list).sort(episodeOrder)
    const first = sorted[0]

    const sizeBytes = sorted.reduce((sum, task) => sum + taskTotalBytes(task), 0)
    const downloadedBytes = sorted.reduce((sum, task) => sum + taskDownloadedBytes(task), 0)
    const knownDownloadedBytes = sorted.reduce(
      (sum, task) => sum + (taskTotalBytes(task) > 0 ? taskDownloadedBytes(task) : 0),
      0,
    )
    /** 未完成且总大小未知的集 */
    const unknownTotal = (task: DownloadTask): boolean =>
      task.status !== 'completed' && taskTotalBytes(task) <= 0
    // 下载中/暂停且总大小未知：这时不能用字节算百分比（分母会漏掉它 → 外层虚高到 100%），
    // 进度按每集等权估算，并标记「不定进度」。
    const indeterminate = sorted.some(
      (task) => unknownTotal(task) && (task.status === 'downloading' || task.status === 'paused'),
    )

    let progress: number
    if (!sorted.some(unknownTotal) && sizeBytes > 0) {
      progress = clampPercent((knownDownloadedBytes / sizeBytes) * 100)
    } else {
      progress = clampPercent(
        sorted.reduce((sum, task) => sum + taskPercent(task), 0) / sorted.length,
      )
    }

    const counts: Record<DownloadStatus, number> = {
      queued: 0,
      downloading: 0,
      paused: 0,
      completed: 0,
      failed: 0,
    }
    for (const task of sorted) counts[task.status] += 1

    const status = STATUS_PRIORITY.find((s) => counts[s] > 0) ?? 'completed'
    const active = counts.queued + counts.downloading + counts.paused

    groups.push({
      key,
      name: first.name || '未命名影片',
      siteName: sorted.find((t) => t.siteName)?.siteName,
      line: first.line,
      cover: sorted.find((t) => t.cover)?.cover,
      tasks: sorted,
      total: sorted.length,
      active,
      downloading: counts.downloading,
      completed: counts.completed,
      failed: counts.failed,
      progress,
      indeterminate,
      speedBps: sorted.reduce(
        (sum, t) => sum + (t.status === 'downloading' ? t.speedBps ?? 0 : 0),
        0,
      ),
      sizeBytes,
      downloadedBytes,
      status,
      latestAt: sorted.reduce((max, t) => Math.max(max, changedAt(t)), 0),
    })
  }

  return groups.sort((a, b) => {
    const aActive = a.active > 0 ? 0 : 1
    const bActive = b.active > 0 ? 0 : 1
    if (aActive !== bActive) return aActive - bActive
    if (b.latestAt !== a.latestAt) return b.latestAt - a.latestAt
    return a.name.localeCompare(b.name, 'zh-Hans-CN')
  })
}

/** 整组状态中文名（卡片头部状态标签） */
export function groupStatusText(group: DownloadSeriesGroup): string {
  switch (group.status) {
    case 'downloading':
      return group.downloading > 1 ? `下载中 ${group.downloading} 集` : '下载中'
    case 'paused':
      return '已暂停'
    case 'queued':
      return '排队中'
    case 'failed':
      return group.failed === group.total ? '全部失败' : '部分失败'
    default:
      return '已完成'
  }
}

/** 速度文案：下载中显示实时速度（0 也显示 0 B/s）；暂停显示 0 B/s；其余为空 */
export function groupSpeedText(group: DownloadSeriesGroup): string {
  if (group.status === 'downloading') {
    const bps = group.speedBps
    return bps && bps > 0 ? `↓ ${formatSpeed(bps)}` : '↓ 0 B/s'
  }
  if (group.status === 'paused') return '↓ 0 B/s'
  return ''
}
