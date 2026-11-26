// ============================================================
// 解析进度组合式函数
// - 订阅后端 `parse-progress` 全局事件（Tauri），按 requestId 过滤本次解析
// - 维护：宏观步骤 / 日志（最多 80 条）/ 本地计时（200ms ticker，与后端 elapsedMs 取 max 对齐）
// - 取消：调 api.cancelParse(requestId)，状态进入 cancelling（尽力而为）
// - 浏览器演示模式（非 Tauri）不 listen，只保留计时与最终结果，避免事件 API 抛错
// 事件契约见 src/types/index.ts 的 ParseProgressEvent；顺序不保证，这里只做追加与容错。
// ============================================================
import { computed, onMounted, onUnmounted, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import { cancelParse, formatError, isTauri } from '@/api'
import type { ParsePhase, ParseProgressEvent, ParseProgressStatus } from '@/types'

/** 一次解析的运行状态：空闲 / 运行中 / 取消中 / 成功 / 失败 */
export type ParseRunState = 'idle' | 'running' | 'cancelling' | 'success' | 'error'

/** 一条解析日志（由 parse-progress 事件追加，UI 直接渲染） */
export interface ParseStep {
  /** 自增 id，用作 v-for key */
  id: number
  phase: ParsePhase
  status: ParseProgressStatus
  title: string
  /** 补充信息（URL / 错误 / 候选数…），已 trim，可能很长 */
  detail: string | null
  /** 后端视角的时间点（毫秒） */
  elapsedMs: number
  /** 前端收到事件的时间（毫秒时间戳） */
  at: number
}

/** 日志上限：超出丢弃最早的条目 */
const MAX_STEPS = 80
/** 本地计时刷新间隔 */
const TICK_MS = 200
/** 生成 requestId：优先 crypto.randomUUID，不可用时用时间戳 + 随机串兜底 */
function createRequestId(): string {
  const c = globalThis.crypto as Crypto | undefined
  if (c && typeof c.randomUUID === 'function') {
    return c.randomUUID()
  }
  return Date.now().toString(36) + Math.random().toString(36).slice(2)
}

/** 事件缺 title 时的兜底文案（后端契约保证有 title，这里只做容错） */
function fallbackTitle(phase: ParsePhase): string {
  switch (phase) {
    case 'prepare':
      return '准备解析'
    case 'parser':
      return '尝试解析器'
    case 'request':
      return '请求解析接口'
    case 'super':
      return '超级解析'
    case 'sniff':
      return '抓包嗅探'
    case 'done':
      return '解析完成'
    case 'error':
      return '解析失败'
    default:
      return '解析进度'
  }
}

export function useParseProgress() {
  /** 运行状态 */
  const state = ref<ParseRunState>('idle')
  /** 当前（最近一次 begin 生成的）请求 id */
  const requestId = ref('')
  /** 日志条目（旧 → 新） */
  const steps = ref<ParseStep[]>([])
  /** 计时（毫秒）：本地 ticker 与后端 elapsedMs 取较大值 */
  const elapsedMs = ref(0)
  /** 失败原因（优先用后端 error 事件 detail，其次页面 finish 传入的文案） */
  const errorMessage = ref('')
  /** 面板标题：剧集名（begin 时传入，可空） */
  const title = ref('')
  /** 是否已被用户关闭 / 成功后自动收起 */
  const dismissed = ref(false)
  /** 最近一条事件的阶段（决定宏观步骤，见 stage） */
  const lastPhase = ref<ParsePhase | null>(null)

  /** 自增日志 id */
  let seq = 0
  /** 本地计时起点 */
  let startedAt = 0
  /** 后端视角的已用时间（只增不减，防止乱序事件回退计时） */
  let backendElapsed = 0
  let ticker: number | null = null
  let autoHideTimer: number | null = null
  let unlisten: UnlistenFn | null = null
  /** 组件已卸载：异步回调里不再改状态 */
  let disposed = false

  /** 当前宏观步骤：0=解析直链 1=抓包嗅探 2=完成/失败 */
  const stage = computed<0 | 1 | 2>(() => {
    if (state.value === 'success' || state.value === 'error') return 2
    if (lastPhase.value === 'sniff') return 1
    if (lastPhase.value === 'done') return 2
    return 0
  })

  /** 面板是否可见（idle 或已关闭时隐藏） */
  const visible = computed(() => state.value !== 'idle' && !dismissed.value)

  /** 最新一条日志（详情页紧凑状态行用） */
  const currentStep = computed<ParseStep | null>(() =>
    steps.value.length ? steps.value[steps.value.length - 1] : null,
  )

  /* ---------------- 计时 ---------------- */

  function stopTicker() {
    if (ticker !== null) {
      window.clearInterval(ticker)
      ticker = null
    }
  }

  function tick() {
    if (state.value !== 'running' && state.value !== 'cancelling') return
    const local = startedAt ? Date.now() - startedAt : 0
    elapsedMs.value = Math.max(local, backendElapsed)
  }

  function startTicker() {
    stopTicker()
    ticker = window.setInterval(tick, TICK_MS)
  }

  function clearAutoHide() {
    if (autoHideTimer !== null) {
      window.clearTimeout(autoHideTimer)
      autoHideTimer = null
    }
  }

  function clearTimers() {
    stopTicker()
    clearAutoHide()
  }

  /* ---------------- 事件 → 日志 ---------------- */

  function pushStep(step: Omit<ParseStep, 'id' | 'at'>): ParseStep {
    const entry: ParseStep = { ...step, id: ++seq, at: Date.now() }
    steps.value.push(entry)
    if (steps.value.length > MAX_STEPS) {
      steps.value.splice(0, steps.value.length - MAX_STEPS)
    }
    return entry
  }

  function applyEvent(ev: ParseProgressEvent) {
    if (!ev || typeof ev !== 'object') return
    // 只处理本次解析的事件：重试 / 切集后旧事件不串台
    if (!requestId.value || ev.requestId !== requestId.value) return
    if (state.value === 'idle') return

    const backendMs = Number.isFinite(ev.elapsedMs) ? Math.max(0, ev.elapsedMs) : 0
    backendElapsed = Math.max(backendElapsed, backendMs)
    // 收到事件时用 max(本地, 后端) 对齐，避免后端时钟更慢导致计时回退
    elapsedMs.value = Math.max(elapsedMs.value, backendElapsed)

    const entry = pushStep({
      phase: ev.phase,
      status: ev.status,
      title: typeof ev.title === 'string' && ev.title.trim() ? ev.title : fallbackTitle(ev.phase),
      detail: typeof ev.detail === 'string' && ev.detail.trim() ? ev.detail.trim() : null,
      elapsedMs: backendElapsed,
    })
    lastPhase.value = entry.phase

    // 只有 phase=error 才是「整次解析失败」；parser/fail 只是某个解析器没命中，继续展示
    if (entry.phase === 'error' && (state.value === 'running' || state.value === 'cancelling')) {
      state.value = 'error'
      errorMessage.value = entry.detail || entry.title || '解析失败'
      stopTicker()
    }
  }

  /* ---------------- 对外方法 ---------------- */

  /** 开始一次解析：重置日志/计时，返回新的 requestId（必须在 invoke 之前调用） */
  function begin(opts: { title?: string } = {}): string {
    clearTimers()
    requestId.value = createRequestId()
    title.value = opts.title?.trim() || ''
    steps.value = []
    elapsedMs.value = 0
    backendElapsed = 0
    startedAt = Date.now()
    errorMessage.value = ''
    dismissed.value = false
    lastPhase.value = null
    state.value = 'running'
    startTicker()
    return requestId.value
  }

  /** 收尾：页面拿到结果 / 抛错后调用（成功后保持展开，由用户手动关闭；失败保留日志） */
  function finish(result: 'success' | 'error', message?: string): void {
    stopTicker()
    if (result === 'success') {
      state.value = 'success'
      errorMessage.value = ''
      // 需求：解析完成后默认不关闭，保持面板可查看（用户可手动关闭）
      return
    }
    state.value = 'error'
    const text = message?.trim()
    if (text) errorMessage.value = text
    else if (!errorMessage.value) errorMessage.value = '解析失败'
  }

  /** 取消本次解析（尽力而为）：后端可能已拿到结果，仍以 resolve/sniff 的返回为准 */
  async function cancel(): Promise<void> {
    if (state.value !== 'running' && state.value !== 'cancelling') return
    const id = requestId.value
    if (!id) return
    state.value = 'cancelling'
    try {
      const accepted = await cancelParse(id)
      // 后端返回 false = 没有匹配到本次解析（令牌已清理 / 刚好完成）。
      // 此时必须退出「取消中…」，否则按钮会一直禁用，用户无法再操作。
      if (!accepted && state.value === 'cancelling') {
        state.value = 'running'
        if (!disposed) {
          pushStep({
            phase: 'prepare',
            status: 'info',
            title: '后端已无此解析（可能即将完成）',
            detail: null,
            elapsedMs: elapsedMs.value,
          })
        }
      }
    } catch (e) {
      // 取消失败不影响解析本身：回到运行态，让用户可以再点一次
      if (state.value === 'cancelling') state.value = 'running'
      if (!disposed) {
        pushStep({
          phase: 'prepare',
          status: 'info',
          title: '取消请求失败',
          detail: formatError(e),
          elapsedMs: elapsedMs.value,
        })
      }
    }
  }

  /** 关闭面板（保留状态与日志；再次 begin 会重新展示） */
  function close(): void {
    clearAutoHide()
    dismissed.value = true
  }

  /** 彻底复位到 idle（切换集 / 使用已有直链时调用） */
  function reset(): void {
    clearTimers()
    state.value = 'idle'
    requestId.value = ''
    steps.value = []
    elapsedMs.value = 0
    backendElapsed = 0
    startedAt = 0
    errorMessage.value = ''
    title.value = ''
    dismissed.value = false
    lastPhase.value = null
  }

  /** 重新打开已收起的解析面板（解析成功自动隐藏后仍可回看日志） */
  function reopen(): void {
    if (state.value === 'idle') return
    clearAutoHide()
    dismissed.value = false
  }

  /* ---------------- 事件订阅（仅 Tauri） ---------------- */

  onMounted(() => {
    if (!isTauri) return
    void listen<ParseProgressEvent>('parse-progress', (event) => {
      if (disposed) return
      applyEvent(event.payload)
    })
      .then((fn) => {
        if (disposed) {
          // 组件在 listen 返回前已卸载：立即解绑，避免泄漏
          fn()
          return
        }
        unlisten = fn
      })
      .catch(() => {
        // 事件总线不可用（例如浏览器预览）时静默降级：面板仍显示计时与最终结果
      })
  })

  onUnmounted(() => {
    disposed = true
    clearTimers()
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  })

  return {
    state,
    requestId,
    steps,
    elapsedMs,
    stage,
    errorMessage,
    title,
    visible,
    currentStep,
    begin,
    finish,
    cancel,
    close,
    reopen,
    reset,
  }
}
