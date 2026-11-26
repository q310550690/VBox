// 下载任务演示数据（mock）：localStorage 持久化 + 前端状态机
// 状态机：queued → downloading → (paused) → completed | failed；cancel 直接移除。
// 仅浏览器预览（demoMode）使用；桌面端走 Rust 下载引擎（src/api/core.ts）。
import type { DownloadAction, DownloadStartPayload, DownloadTask } from '@/types'

const STORAGE_KEY = 'vbox.downloads.v2'

const DEMO_TASKS: DownloadTask[] = [
  {
    id: 'dl-demo-1',
    name: '长夜将明',
    epTitle: '第3集',
    siteKey: 'demo_cms_cuiwei',
    line: 'hhyun',
    cover: 'https://picsum.photos/seed/vbox-1-0/300/450',
    status: 'downloading',
    progress: 62,
    speedBps: 8.6 * 1024 * 1024,
    sizeBytes: 1.4 * 1024 * 1024 * 1024,
    createdAt: Date.now() - 1000 * 60 * 12,
  },
  {
    id: 'dl-demo-2',
    name: '雾城疑云',
    epTitle: '第8集',
    siteKey: 'demo_cms_shuangyue',
    line: 'hhm3u8',
    cover: 'https://picsum.photos/seed/vbox-2-3/300/450',
    status: 'paused',
    progress: 35,
    speedBps: 0,
    sizeBytes: 2.1 * 1024 * 1024 * 1024,
    createdAt: Date.now() - 1000 * 60 * 40,
  },
  {
    id: 'dl-demo-3',
    name: '盛夏回声',
    epTitle: '第1集',
    siteKey: 'demo_cms_cuiwei',
    line: 'hhyun',
    cover: 'https://picsum.photos/seed/vbox-1-19/300/450',
    status: 'queued',
    progress: 0,
    sizeBytes: 860 * 1024 * 1024,
    createdAt: Date.now() - 1000 * 60 * 2,
  },
  {
    id: 'dl-demo-4',
    name: '深海纪元',
    epTitle: '第1集',
    siteKey: 'demo_cms_cuiwei',
    line: '夸克网盘',
    cover: 'https://picsum.photos/seed/vbox-1-7/300/450',
    status: 'completed',
    progress: 100,
    sizeBytes: 1.2 * 1024 * 1024 * 1024,
    downloadedBytes: 1.2 * 1024 * 1024 * 1024,
    filePath: 'C:\\Users\\Public\\VBox\\downloads\\深海纪元\\第1集.mp4',
    kind: 'file',
    createdAt: Date.now() - 1000 * 60 * 60 * 26,
  },
  {
    id: 'dl-demo-5',
    name: '星轨之外',
    epTitle: '第2集',
    siteKey: 'demo_cms_yanshan',
    line: 'hhm3u8',
    cover: 'https://picsum.photos/seed/vbox-1-9/300/450',
    status: 'failed',
    progress: 12,
    sizeBytes: 980 * 1024 * 1024,
    error: '演示错误：连接被重置',
    createdAt: Date.now() - 1000 * 60 * 60 * 3,
  },
  {
    id: 'dl-demo-6',
    name: '山海之间',
    epTitle: '第4集',
    siteKey: 'demo_cms_qinghe',
    line: 'hhm3u8',
    cover: 'https://picsum.photos/seed/vbox-2-72/300/450',
    status: 'downloading',
    progress: 18,
    speedBps: 5.2 * 1024 * 1024,
    sizeBytes: 1.1 * 1024 * 1024 * 1024,
    createdAt: Date.now() - 1000 * 60 * 5,
  },
  {
    id: 'dl-demo-7',
    name: '长安三万里',
    epTitle: '正片',
    siteKey: 'demo_cms_xingchen',
    line: 'hhyun',
    cover: 'https://picsum.photos/seed/vbox-1-53/300/450',
    status: 'queued',
    progress: 0,
    sizeBytes: 2.8 * 1024 * 1024 * 1024,
    createdAt: Date.now() - 1000 * 60 * 1,
  },
  {
    id: 'dl-demo-8',
    name: '灵笼',
    epTitle: '第7集',
    siteKey: 'demo_cms_haijiao',
    line: 'hhm3u8',
    cover: 'https://picsum.photos/seed/vbox-4-58/300/450',
    status: 'completed',
    progress: 100,
    sizeBytes: 760 * 1024 * 1024,
    downloadedBytes: 760 * 1024 * 1024,
    filePath: 'C:\\Users\\Public\\VBox\\downloads\\灵笼\\第7集.mp4',
    kind: 'file',
    createdAt: Date.now() - 1000 * 60 * 60 * 12,
  },
]

interface Persisted {
  seeded: boolean
  tasks: DownloadTask[]
}

function read(): Persisted {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const data = JSON.parse(raw) as Persisted
      if (Array.isArray(data.tasks)) return data
    }
  } catch {
    /* 忽略损坏数据 */
  }
  const initial: Persisted = { seeded: true, tasks: DEMO_TASKS }
  write(initial)
  return initial
}

function write(data: Persisted) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(data))
  } catch {
    /* 存储不可用时忽略 */
  }
}

export function loadDownloads(): DownloadTask[] {
  return read().tasks
}

export function saveDownloads(tasks: DownloadTask[]): DownloadTask[] {
  write({ seeded: true, tasks })
  return tasks
}

/** 演示模式新建任务：不入队解析，直接按已解析直链生成一条 queued 任务 */
export function mockDownloadStart(payload: DownloadStartPayload): DownloadTask {
  const data = read()
  const now = Date.now()
  const task: DownloadTask = {
    // 形如 dl-demo-<ts>；同毫秒内多次入队时追加随机后缀避免 id 冲突
    id: `dl-demo-${now}-${Math.random().toString(36).slice(2, 6)}`,
    name: payload.name,
    epTitle: payload.epTitle,
    siteKey: payload.siteKey,
    siteName: payload.siteName,
    line: payload.line,
    vodId: payload.vodId,
    epIndex: payload.epIndex,
    cover: payload.cover,
    status: 'queued',
    progress: 0,
    sizeBytes: (0.8 + Math.random() * 0.8) * 1024 * 1024 * 1024,
    kind: /\.m3u8(\?|$)/i.test(payload.url) ? 'hls' : 'file',
    createdAt: now,
    updatedAt: now,
  }
  // 与真实后端一致：createdAt 倒序
  data.tasks = [task, ...data.tasks]
  write(data)
  return task
}

/** 演示模式任务控制：与 store 的 1s ticker 配合推进状态机 */
export function mockDownloadControl(id: string, action: DownloadAction): DownloadTask[] {
  const data = read()
  const task = data.tasks.find((t) => t.id === id)
  if (!task) return data.tasks
  switch (action) {
    case 'pause':
      if (task.status === 'downloading' || task.status === 'queued') {
        task.status = 'paused'
        task.speedBps = 0
      }
      break
    case 'resume':
      if (task.status === 'paused' || task.status === 'failed') {
        task.status = 'downloading'
        task.speedBps = 6 * 1024 * 1024
        task.error = undefined
      }
      break
    case 'cancel':
      data.tasks = data.tasks.filter((t) => t.id !== id)
      break
    case 'retry':
      if (task.status === 'failed') {
        task.status = 'queued'
        task.progress = 0
        task.speedBps = undefined
        task.error = undefined
      }
      break
  }
  task.updatedAt = Date.now()
  write(data)
  return data.tasks
}

/** 演示模式删除任务记录（不涉及真实文件） */
export function mockDownloadRemove(id: string): DownloadTask[] {
  const data = read()
  data.tasks = data.tasks.filter((t) => t.id !== id)
  write(data)
  return data.tasks
}
