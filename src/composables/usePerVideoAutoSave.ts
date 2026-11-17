// ============================================================
// 单片「边下边播」覆盖开关（播放器控制条）
// - 设置页 downloads.settings.autoSave 是全局总开关；
// - 本文件按「站点 + 影片」保存覆盖值：未覆盖时跟随全局，覆盖后以本片为准；
// - 覆盖表持久化在 localStorage：键 vbox.player.autoSaveOverrides，
//   值形如 { "<siteKey>/<vodId>": boolean }；
// - UI 组件只通过本 composable 读写，不直接碰 localStorage 细节。
// ============================================================
import { computed, ref } from 'vue'
import { useDownloadsStore } from '@/store/downloads'

/** 覆盖表存储键：{ "<siteKey>/<vodId>": boolean } */
export const AUTOSAVE_OVERRIDES_KEY = 'vbox.player.autoSaveOverrides'

/** 读取覆盖表：容错解析，只保留 boolean 值 */
function readOverrides(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(AUTOSAVE_OVERRIDES_KEY)
    const parsed = raw ? (JSON.parse(raw) as unknown) : null
    if (!parsed || typeof parsed !== 'object') return {}
    const next: Record<string, boolean> = {}
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof value === 'boolean') next[key] = value
    }
    return next
  } catch {
    return {}
  }
}

/** 覆盖表（模块级共享：同一页面的多个读取方拿到同一份响应式状态） */
const overrides = ref<Record<string, boolean>>(readOverrides())

/** 写回 localStorage（不可用时仅保留内存态，不打断播放） */
function writeOverrides() {
  try {
    localStorage.setItem(AUTOSAVE_OVERRIDES_KEY, JSON.stringify(overrides.value))
  } catch {
    // localStorage 不可用（隐私模式等）：仅内存态
  }
}

/**
 * 单片「边下边播」开关。
 * @param videoKey 当前影片 key 的 getter（`<siteKey>/<vodId>`，路由/详情变化后自动跟随）
 */
export function usePerVideoAutoSave(videoKey: () => string) {
  const downloads = useDownloadsStore()

  /** 本片覆盖值：undefined = 未覆盖（跟随全局开关） */
  const override = computed<boolean | undefined>(() => {
    const value = overrides.value[videoKey()]
    return typeof value === 'boolean' ? value : undefined
  })

  /** 本片是否已单独覆盖 */
  const hasOverride = computed(() => override.value !== undefined)

  /** 生效值：本片覆盖优先，否则跟随设置页全局「播放时自动保存」 */
  const effective = computed(() => override.value ?? downloads.settings.autoSave)

  /** 写入本片覆盖值并持久化 */
  function setOverride(enabled: boolean) {
    const key = videoKey()
    if (!key) return
    overrides.value = { ...overrides.value, [key]: enabled }
    writeOverrides()
  }

  /** 按当前生效值翻转本片开关，返回翻转后的值 */
  function toggle(): boolean {
    const next = !effective.value
    setOverride(next)
    return next
  }

  return { effective, override, hasOverride, setOverride, toggle }
}
