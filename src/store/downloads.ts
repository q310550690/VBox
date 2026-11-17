// 下载任务状态（Pinia）：桌面端走 Rust 下载引擎（invoke + download-progress 事件）；
// 浏览器预览（demoMode）走 mock + localStorage，并保留 1s 演示状态机 ticker。
// 状态机：queued → downloading → (paused) → completed | failed；cancel 直接移除。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type {
  DownloadAction,
  DownloadSettings,
  DownloadStartPayload,
  DownloadTask,
} from '@/types'

const TICK_MS = 1000

/** 后端未就绪时的目录占位（真实值由 download_settings 返回） */
const DEFAULT_SETTINGS: DownloadSettings = {
  dir: '',
  defaultDir: '',
  autoSave: false,
  concurrency: 1,
  speedLimitKbps: 0,
  autoSaveLowPriority: true,
  autoSaveLimitKbps: 0,
}

/** 事件只订阅一次：store 常驻，页面卸载不取消（避免多次进入下载页重复监听） */
let unlistenProgress: (() => void) | null = null

export const useDownloadsStore = defineStore('downloads', {
  state: () => ({
    tasks: [] as DownloadTask[],
    /** 下载/缓存目录设置（设置页「下载与缓存」、下载页页头展示） */
    settings: { ...DEFAULT_SETTINGS } as DownloadSettings,
    loading: false,
    settingsLoading: false,
    /** 设置是否已成功/尝试加载过（播放自动保存判断前会等它） */
    settingsLoaded: false,
    /** 是否已订阅后端 download-progress 事件（仅 Tauri） */
    listening: false,
    initialized: false,
    error: '',
    tickerRunning: false,
  }),

  getters: {
    activeTasks(state): DownloadTask[] {
      return state.tasks.filter(
        (t) => t.status === 'downloading' || t.status === 'queued' || t.status === 'paused',
      )
    },
    completedTasks(state): DownloadTask[] {
      return state.tasks.filter((t) => t.status === 'completed')
    },
    failedTasks(state): DownloadTask[] {
      return state.tasks.filter((t) => t.status === 'failed')
    },
    counts(state) {
      return {
        downloading: state.tasks.filter((t) => t.status === 'downloading').length,
        active: state.tasks.filter(
          (t) => t.status === 'downloading' || t.status === 'queued' || t.status === 'paused',
        ).length,
        completed: state.tasks.filter((t) => t.status === 'completed').length,
        failed: state.tasks.filter((t) => t.status === 'failed').length,
      }
    },
    /** 所有下载中任务的实时总速度（B/s）；暂停/排队不贡献速度 */
    totalSpeedBps(state): number {
      return state.tasks.reduce(
        (sum, task) => sum + (task.status === 'downloading' ? task.speedBps ?? 0 : 0),
        0,
      )
    },
    isEmpty(state): boolean {
      return state.tasks.length === 0
    },
  },

  actions: {
    async load() {
      if (this.loading) return
      this.loading = true
      this.error = ''
      try {
        this.tasks = await api.downloadList()
      } catch (e) {
        this.error = api.formatError(e)
      } finally {
        this.loading = false
        this.initialized = true
      }
      // 目录设置与列表并行加载；设置失败不阻断列表
      void this.loadSettings()
      if (api.demoMode) this.ensureTicker()
      else await this.ensureListener()
    },

    /** 重新拉取任务列表（新建 / 控制后由后端返回列表覆盖的补充入口） */
    async refresh() {
      try {
        this.tasks = await api.downloadList()
        this.error = ''
      } catch (e) {
        this.error = api.formatError(e)
      }
    },

    /** 读取下载目录 / 自动保存设置（设置页 / 下载页页头 / 播放自动保存） */
    async loadSettings() {
      if (this.settingsLoading) return
      this.settingsLoading = true
      try {
        this.settings = await api.downloadSettings()
      } catch (e) {
        this.error = api.formatError(e)
      } finally {
        this.settingsLoading = false
        this.settingsLoaded = true
      }
    },

    /** 切换「播放时自动保存」；后端持久化并返回最新设置 */
    async updateAutoSave(enabled: boolean) {
      this.settings = await api.downloadSetAutoSave(enabled)
    },

    /** 更新下载并发数 / 全局限速；后端持久化并返回最新设置 */
    async updateNetwork(concurrency: number, speedLimitKbps: number) {
      this.settings = await api.downloadSetNetwork(concurrency, speedLimitKbps)
    },

    /** 更新自动保存任务策略（低优先级 / 单独限速）；后端持久化并返回最新设置 */
    async updateAutoSavePolicy(lowPriority: boolean, autoSaveLimitKbps: number) {
      this.settings = await api.downloadSetAutoSavePolicy(lowPriority, autoSaveLimitKbps)
    },

    /** Tauri：订阅 download-progress（载荷=单个 DownloadTask，按 id upsert） */
    async ensureListener() {
      if (this.listening || api.demoMode || unlistenProgress) {
        this.listening = Boolean(unlistenProgress)
        return
      }
      this.listening = true
      try {
        const { listen } = await import('@tauri-apps/api/event')
        unlistenProgress = await listen<DownloadTask>('download-progress', (event) => {
          const task = event.payload
          if (!task || !task.id) return
          const index = this.tasks.findIndex((t) => t.id === task.id)
          if (index >= 0) {
            // 直接替换整条任务：后端下发的是完整快照（进度/速度/状态/错误）
            this.tasks[index] = task
          } else {
            this.tasks.push(task)
            this.tasks.sort((a, b) => b.createdAt - a.createdAt)
          }
        })
      } catch (e) {
        this.listening = false
        this.error = api.formatError(e)
      }
    },

    /** 新建下载任务（payload.url 必须已解析为直链），成功后刷新列表 */
    async start(payload: DownloadStartPayload): Promise<DownloadTask> {
      const task = await api.downloadStart(payload)
      await this.refresh()
      return task
    },

    /** 任务控制：暂停 / 继续 / 取消 / 重试；返回是否成功（失败文案见 store.error） */
    async control(id: string, action: DownloadAction): Promise<boolean> {
      try {
        this.tasks = await api.downloadControl(id, action)
        this.error = ''
        return true
      } catch (e) {
        this.error = api.formatError(e)
        return false
      }
    },

    /** 删除任务记录；removeFile=false 时保留已下载文件 */
    async remove(id: string, removeFile = false): Promise<boolean> {
      try {
        this.tasks = await api.downloadRemove(id, removeFile)
        this.error = ''
        return true
      } catch (e) {
        this.error = api.formatError(e)
        return false
      }
    },

    /** 批量控制：同一个 action 依次作用到多个任务（后端无批量命令） */
    async controlMany(ids: string[], action: DownloadAction): Promise<boolean> {
      if (!ids.length) return true
      try {
        for (const id of ids) {
          this.tasks = await api.downloadControl(id, action)
        }
        this.error = ''
        return true
      } catch (e) {
        this.error = api.formatError(e)
        return false
      }
    },

    /** 批量「下载」：已暂停的继续、失败的重新入队；排队/下载中/已完成不动 */
    async downloadMany(ids: string[]): Promise<boolean> {
      if (!ids.length) return true
      try {
        for (const id of ids) {
          const task = this.tasks.find((t) => t.id === id)
          if (!task) continue
          if (task.status === 'paused') {
            this.tasks = await api.downloadControl(id, 'resume')
          } else if (task.status === 'failed') {
            this.tasks = await api.downloadControl(id, 'retry')
          }
        }
        this.error = ''
        return true
      } catch (e) {
        this.error = api.formatError(e)
        return false
      }
    },

    /** 全部下载：继续所有已暂停 + 重试所有失败（排队/下载中无需操作） */
    async downloadAll(): Promise<boolean> {
      const ids = this.tasks
        .filter((t) => t.status === 'paused' || t.status === 'failed')
        .map((t) => t.id)
      return this.downloadMany(ids)
    },

    /** 暂停全部进行中任务（后端无批量命令，逐个调用并覆盖列表） */
    async pauseAll(): Promise<boolean> {
      const ids = this.tasks
        .filter((t) => t.status === 'downloading' || t.status === 'queued')
        .map((t) => t.id)
      return this.controlMany(ids, 'pause')
    },

    /* ---------------- 仅演示模式：1s 前端状态机 ---------------- */

    ensureTicker() {
      if (!api.demoMode || this.tickerRunning) return
      this.tickerRunning = true
      window.setInterval(() => this.tick(), TICK_MS)
    },

    /** 演示用状态机推进：每 1s 增加进度；没有下载中任务时自动启动队列 */
    tick() {
      // Tauri 下进度完全由后端事件驱动，绝不跑演示 ticker
      if (!api.demoMode) return
      let changed = false
      for (const t of this.tasks) {
        if (t.status !== 'downloading') continue
        t.progress = Math.min(100, t.progress + 1 + Math.random() * 2)
        t.speedBps = (6 + Math.random() * 6) * 1024 * 1024
        changed = true
        if (t.progress >= 100) {
          t.progress = 100
          t.status = 'completed'
          t.speedBps = 0
        }
      }
      if (!this.tasks.some((t) => t.status === 'downloading')) {
        const next = this.tasks.find((t) => t.status === 'queued')
        if (next) {
          next.status = 'downloading'
          changed = true
        }
      }
      if (changed) void api.downloadSave(this.tasks)
    },
  },
})
