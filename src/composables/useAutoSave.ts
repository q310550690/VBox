// 播放时自动保存（方案 1）：
// - HLS：交给本地代理的「边看边存」——只在当前 HLS 会话上登记导出目标，
//   播放结束/切集时后台复用已缓存分片补齐并落盘（单份流量），不再另起下载任务；
// - 非 HLS（mp4 直链等）：仍走原下载队列（后台拉一份完整文件，后端同集去重）。
import * as api from '@/api'
import { useDownloadsStore } from '@/store/downloads'
import type { DownloadStartPayload } from '@/types'

export function useAutoSave() {
  const downloads = useDownloadsStore()

  /**
   * 尝试保存正在播放的剧集。
   * - 浏览器预览（demoMode）不触发，避免生成演示任务；
   * - enabledOverride：true = 强制执行（本片「边下边播」开启），false = 强制跳过（本片关闭），
   *   undefined = 沿用原逻辑（等设置加载完，读全局 downloads.settings.autoSave）；
   * - HLS 优先用代理导出（返回 false 才回退下载队列）；
   * - 失败只写 console，不打断播放（下载页仍可手动下载/播放器可手动保存）。
   */
  async function autoSave(payload: DownloadStartPayload, enabledOverride?: boolean): Promise<void> {
    if (api.demoMode) return
    try {
      // 本片显式关闭：直接跳过，不必等设置加载
      if (enabledOverride === false) return
      // 未指定覆盖值：沿用全局开关（等一次设置加载）
      if (enabledOverride === undefined) {
        if (!downloads.settingsLoaded) await downloads.loadSettings()
        if (!downloads.settings.autoSave) return
      }

      // HLS：登记到本地代理，避免「边下边播」和下载引擎各拉一份
      if (payload.url.toLowerCase().includes('.m3u8')) {
        const armed = await api.playerArmAutoSave(payload)
        if (armed) return
      }

      // 非 HLS：标记为低优先级自动保存任务，普通下载进行时让路，并按策略限速
      await downloads.start({ ...payload, priority: 1 })
      // 提前订阅进度事件：用户切到下载页时能立刻看到进度（幂等）
      void downloads.ensureListener()
    } catch (e) {
      console.warn('[VBox] 播放时自动保存失败：', api.formatError(e))
    }
  }

  return { autoSave }
}
