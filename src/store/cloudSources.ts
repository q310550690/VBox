// 云盘搜索源状态（W14 §7.3）：内置内核 / PanSou 状态、搜索源优先级，供设置页与后续页面复用。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { CloudSourcePriority } from '@/api/cloud'
import type { CloudSourceStatus } from '@/api/cloud'

function asPriority(value: string): CloudSourcePriority | null {
  return value === 'builtin' || value === 'builtin_only' || value === 'pansou_only' ? value : null
}

export const useCloudSourcesStore = defineStore('cloudSources', {
  state: () => ({
    statuses: [] as CloudSourceStatus[],
    priority: 'builtin' as CloudSourcePriority,
    loading: false,
    savingPriority: false,
    loaded: false,
  }),
  getters: {
    builtin(state): CloudSourceStatus | undefined {
      return state.statuses.find((item) => item.kind === 'builtin_kernel')
    },
    pansou(state): CloudSourceStatus | undefined {
      return state.statuses.find((item) => item.kind === 'pan_sou')
    },
    /** 只有 PanSou 就绪时才允许「只使用增强源」 */
    pansouConfigured(): boolean {
      return this.pansou?.ready === true
    },
  },
  actions: {
    setStatuses(statuses: CloudSourceStatus[]) {
      this.statuses = statuses
      this.loaded = true
    },
    async load(force = false) {
      if (this.loading) return
      if (!force && this.loaded) return
      this.loading = true
      try {
        const [statuses, priority] = await Promise.all([
          api.cloudSourceStatus(),
          api.cloudSourcesPriority(),
        ])
        this.statuses = statuses
        const next = asPriority(priority)
        if (next) this.priority = next
        this.loaded = true
      } finally {
        this.loading = false
      }
    },
    /** 切换优先级：成功后后端返回最新源状态 */
    async setPriority(value: CloudSourcePriority) {
      if (this.savingPriority || value === this.priority) return
      const previous = this.priority
      this.savingPriority = true
      try {
        this.statuses = await api.cloudSourcePrioritySet(value)
        this.priority = value
      } catch (e) {
        this.priority = previous
        throw e
      } finally {
        this.savingPriority = false
      }
    },
  },
})
