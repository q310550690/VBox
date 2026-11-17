import { nextTick, ref } from 'vue'

/**
 * DetailView 是否参与 VodLayout 的 KeepAlive 缓存。
 *
 * 侧栏切到设置/历史等页面时保持缓存，回来仍能复用详情页状态；
 * 详情页自身的「返回」按钮则调用 evictDetailView() 把它从缓存中剔除，
 * 确保返回后页面真正销毁、播放器和定时器不再留在后台。
 */
export const detailCached = ref(true)

/** 从 KeepAlive 中剔除 DetailView，下一个 tick 恢复 include，避免影响后续访问。 */
export function evictDetailView(): void {
  if (!detailCached.value) return
  detailCached.value = false
  void nextTick(() => {
    detailCached.value = true
  })
}
