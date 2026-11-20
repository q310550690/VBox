<!-- 解析进度面板（详情页内联 / 其它场景悬浮）
     - 数据全部来自 useParseProgress()：state / steps / elapsedMs / stage / errorMessage
     - 默认 fixed 悬浮在内容区右下角；传 inline 后作为普通卡片嵌进父级 grid（详情页选集右侧 4 列）
     - 只用 styles/tokens.css 的变量，亮/暗主题自动适配
     - 事件只做展示：解析日志追加、自动滚底、最多保留 80 条（组合式函数内裁剪） -->
<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import type { ScrollbarInstance } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import { detectCatvodErrorCode } from '@/utils/catvodErrors'
import type { SolarIconName } from '@/icons/solar'
import type { ParseRunState, ParseStep } from '@/composables/useParseProgress'

const props = defineProps<{
  /** 是否显示面板（idle / 已关闭 / 成功后自动收起时为 false） */
  visible: boolean
  /** 内联模式：不再 fixed 悬浮，而是作为普通块级卡片嵌在页面布局里（详情页选集右侧） */
  inline?: boolean
  /** 运行状态 */
  state: ParseRunState
  /** 剧集名（可空，头部省略号截断） */
  title: string
  /** 解析日志（旧 → 新） */
  steps: ParseStep[]
  /** 计时（毫秒） */
  elapsedMs: number
  /** 当前宏观步骤：0=解析直链 1=抓包嗅探 2=完成/失败 */
  stage: 0 | 1 | 2
  /** 失败原因 */
  errorMessage: string
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'retry'): void
  (e: 'close'): void
}>()

const router = useRouter()

/**
 * 网盘类失败（W13 未登录联动）：提示去「云盘 → 右上角账号管理」登录后重试。
 * 后端把缺失 Cookie/登录态归为 `catvod.pan.cookie_required`，部分老文案直接带 login_required。
 */
const panLoginHint = computed(() => {
  const text = props.errorMessage || ''
  return detectCatvodErrorCode(text) === 'catvod.pan.cookie_required' || /login_required|需要登录/.test(text)
})

function goPanLogin() {
  void router.push({ path: '/cloud', query: { accounts: '1' } })
}

/* ---------------- 宏观步骤 ---------------- */

type StepVisual = 'waiting' | 'active' | 'done' | 'failed' | 'skipped'

interface MacroStep {
  key: 0 | 1 | 2
  label: string
  hint: string
}

const MACRO_STEPS: MacroStep[] = [
  { key: 0, label: '解析直链', hint: '解析器 / 解析接口 / 超级解析' },
  { key: 1, label: '抓包嗅探', hint: '隐藏 WebView 捕获媒体地址；不需要时显示「跳过」' },
  { key: 2, label: '完成 / 失败', hint: '拿到播放地址或失败原因' },
]

const RESOLVE_PHASES = ['prepare', 'parser', 'request', 'super']

const isTerminal = computed(() => props.state === 'success' || props.state === 'error')
const isRunning = computed(() => props.state === 'running' || props.state === 'cancelling')
const isCancelling = computed(() => props.state === 'cancelling')

const sniffSteps = computed(() => props.steps.filter((s) => s.phase === 'sniff'))
const sniffStarted = computed(() => sniffSteps.value.length > 0)
const sniffOk = computed(() => sniffSteps.value.some((s) => s.status === 'ok'))
const sniffFailed = computed(() => sniffSteps.value.some((s) => s.status === 'fail'))

/** ① 解析直链：有 ok 即完成；error 且从未进入抓包 → 失败；有事件但未终态 → 进行中 */
const resolveVisual = computed<StepVisual>(() => {
  const related = props.steps.filter((s) => RESOLVE_PHASES.includes(s.phase))
  if (related.some((s) => s.status === 'ok')) return 'done'
  if (sniffStarted.value || props.state === 'success') return 'done'
  if (props.state === 'error') return 'failed'
  if (related.length > 0) return 'active'
  // 还没有任何事件：等待后端返回（不伪造成功步骤）
  return isRunning.value ? 'active' : 'waiting'
})

/** ② 抓包嗅探：未进入抓包就结束 → 跳过 */
const sniffVisual = computed<StepVisual>(() => {
  if (sniffFailed.value) return 'failed'
  if (sniffOk.value) return 'done'
  if (sniffStarted.value) return props.state === 'error' ? 'failed' : 'active'
  if (isTerminal.value) return 'skipped'
  return 'waiting'
})

/** ③ 完成 / 失败 */
const doneVisual = computed<StepVisual>(() => {
  if (props.state === 'success') return 'done'
  if (props.state === 'error') return 'failed'
  if (props.stage === 2) return 'active'
  return 'waiting'
})

const visuals = computed<StepVisual[]>(() => [
  resolveVisual.value,
  sniffVisual.value,
  doneVisual.value,
])

function visualText(visual: StepVisual): string {
  switch (visual) {
    case 'active':
      return '进行中'
    case 'done':
      return '已完成'
    case 'failed':
      return '失败'
    case 'skipped':
      return '跳过'
    default:
      return '等待'
  }
}

const VISUAL_ICONS: Record<StepVisual, SolarIconName> = {
  waiting: 'minus',
  skipped: 'minus',
  active: 'clock-circle',
  done: 'check-circle',
  failed: 'close-circle',
}

/* ---------------- 日志 ---------------- */

const LOG_ICONS: Record<ParseStep['status'], SolarIconName> = {
  start: 'clock-circle',
  ok: 'check-circle',
  fail: 'close-circle',
  info: 'info-circle',
}

/** 日志滚动容器（el-scrollbar 实例：自动滚到底部 / 保留用户横向滚动位置） */
const scrollRef = ref<ScrollbarInstance | null>(null)
/** 刚复制过的日志行 id（短暂显示对勾） */
const copiedId = ref<number | null>(null)
let copiedTimer: number | null = null

/** 复制单行日志（时间 + 标题 + 详情） */
async function copyStep(step: ParseStep) {
  const text = [
    stepTime(step.elapsedMs),
    step.title,
    step.detail ?? '',
  ]
    .filter(Boolean)
    .join(' ')
  try {
    await navigator.clipboard.writeText(text)
    copiedId.value = step.id
    if (copiedTimer !== null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copiedId.value = null
      copiedTimer = null
    }, 1200)
  } catch {
    // 剪贴板不可用：静默忽略（不阻断解析流程）
  }
}

function scrollToBottom() {
  void nextTick(() => {
    // 只动纵向：传足够大的值，内部会按实际 scrollHeight 截断；横向滚动位置不受影响
    scrollRef.value?.setScrollTop(Number.MAX_SAFE_INTEGER)
  })
}

watch(
  () => props.steps.length,
  () => scrollToBottom(),
)
watch(
  () => props.visible,
  (visible) => {
    if (visible) scrollToBottom()
  },
)

/** 无事件时的占位文案：按状态给不同说明，绝不显示虚假的成功步骤 */
const emptyLogText = computed(() => {
  if (props.state === 'success') return '解析完成（后端未推送进度事件）'
  if (props.state === 'error') return props.errorMessage || '解析失败（后端未推送进度事件）'
  return '已发起解析，等待后端返回…'
})

/* ---------------- 格式化 ---------------- */

/** 头部计时：12.4s（超过 1 分钟显示 1m02.4s） */
const timeText = computed(() => formatSeconds(props.elapsedMs))

function formatSeconds(ms: number): string {
  const total = Math.max(0, ms) / 1000
  if (total < 60) return `${total.toFixed(1)}s`
  const minutes = Math.floor(total / 60)
  const seconds = total - minutes * 60
  return `${minutes}m${seconds.toFixed(1).padStart(4, '0')}s`
}

/** 日志行时间：+1.2s */
function stepTime(ms: number): string {
  return `+${(Math.max(0, ms) / 1000).toFixed(1)}s`
}

/* ---------------- 操作 ---------------- */

function handleCancel() {
  if (isCancelling.value) return
  emit('cancel')
}
</script>

<template>
  <aside
    v-if="visible"
    class="parse-panel"
    :class="{ 'is-inline': inline }"
    role="status"
    aria-live="polite"
    aria-atomic="false"
    aria-label="解析进度"
  >
      <!-- 头部：标题 + 剧集名 + 总用时 + 操作（解析中显示取消） -->
      <header class="panel-head">
        <span class="head-title">解析进度</span>
        <span v-if="title" class="head-ep ellipsis-1" :title="title">{{ title }}</span>
        <span class="head-time" aria-live="off" :title="`已用时 ${elapsedMs} 毫秒`">{{ timeText }}</span>

        <!-- 操作按钮紧跟解析总用时后面 -->
        <div v-if="isRunning" class="panel-actions">
          <el-button size="small" :loading="isCancelling" :disabled="isCancelling" @click="handleCancel">
            {{ isCancelling ? '取消中…' : '取消解析' }}
          </el-button>
        </div>

        <!-- 圆形重试图标：只在解析失败时出现（成功 / 进行中都不显示） -->
        <button
          v-if="state === 'error'"
          type="button"
          class="head-retry"
          title="重新解析"
          aria-label="重新解析"
          @click="emit('retry')"
        >
          <SolarIcon name="refresh" :size="12" />
        </button>
      </header>

      <!-- 三个宏观步骤 -->
      <ol class="steps" :aria-label="`当前步骤：${MACRO_STEPS[stage]?.label ?? ''}`">
        <li
          v-for="(step, index) in MACRO_STEPS"
          :key="step.key"
          class="step"
          :class="`is-${visuals[index]}`"
          :title="step.hint"
        >
          <span class="step-dot">
            <SolarIcon
              :name="VISUAL_ICONS[visuals[index]]"
              :size="13"
            />
          </span>
          <span class="step-label">{{ step.label }}</span>
          <span class="step-status">{{ visualText(visuals[index]) }}</span>
        </li>
      </ol>

      <!-- 解析日志：最多 6 行可见，自动滚到底部；复制按钮 sticky 在可视区右侧（表格 fixed 列手感） -->
      <div class="log-head">
        <span>解析日志</span>
        <span v-if="steps.length" class="log-count">{{ steps.length }} 条</span>
      </div>
      <el-scrollbar ref="scrollRef" class="log-scroll" max-height="200px">
        <div class="log" role="log">
          <p v-if="!steps.length" class="log-empty">
            <SolarIcon name="clock-circle" :size="12" />
            <span class="ellipsis-1" :title="emptyLogText">{{ emptyLogText }}</span>
          </p>
          <div
            v-for="step in steps"
            :key="step.id"
            class="log-row"
            :class="`is-${step.status}`"
          >
            <SolarIcon
              :name="LOG_ICONS[step.status]"
              :size="12"
              class="log-icon"
            />
            <span class="log-title" :title="step.title">{{ step.title }}</span>
            <span v-if="step.detail" class="log-detail" :title="step.detail">{{ step.detail }}</span>
            <!-- 右侧固定列：时间 + 复制，横向滚动时贴住可视区右边 -->
            <span class="log-meta">
              <span class="log-time">{{ stepTime(step.elapsedMs) }}</span>
              <button
                type="button"
                class="log-copy"
                :title="copiedId === step.id ? '已复制' : '复制这一行'"
                :aria-label="copiedId === step.id ? '已复制' : '复制这一行'"
                @click.stop="copyStep(step)"
              >
                <SolarIcon :name="copiedId === step.id ? 'check-circle' : 'copy'" :size="12" />
              </button>
            </span>
          </div>
        </div>
      </el-scrollbar>

      <!-- 失败原因（后端 error detail 或页面 catch 文案）；网盘未登录时给「去登录」入口 -->
      <div v-if="state === 'error' && errorMessage" class="error-block">
        <p class="error-text ellipsis-2" :title="errorMessage">
          {{ errorMessage }}
        </p>
        <el-button
          v-if="panLoginHint"
          size="small"
          type="primary"
          @click="goPanLogin"
        >
          去登录网盘账号
        </el-button>
      </div>
    </aside>
</template>

<style scoped>
.parse-panel {
  position: fixed;
  right: var(--space-4);
  bottom: var(--space-4);
  /* 高于播放器浮层 / 外壳，低于全局消息层（2000+），既能盖住视频又不遮 toast */
  z-index: 900;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  width: 400px;
  /* width: min(680px, calc(100vw - 48px)); */
  max-width: calc(100vw - 48px);
  max-height: calc(100vh - 120px);
  padding: var(--space-3);
  background: var(--surface-card);
  border: 0;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-3);
}

/* ---------------- 内联模式：和选集并排的普通卡片（详情页） ---------------- */

/* 不再脱离文档流：作为 grid 的第 2 列，宽度由父级 4fr 决定；
   高度撑满父级这一行（与选集卡片等高），日志区吃掉剩余空间 */
.parse-panel.is-inline {
  position: static;
  inset: auto;
  z-index: auto;
  align-self: stretch;
  width: 100%;
  max-width: none;
  max-height: none;
  min-height: 0;
  box-shadow: none;
}

/* 日志滚动区：flex 占满头部/步骤/操作之后的空间，内容多时内部滚动 */
.parse-panel.is-inline .log-scroll {
  flex: 1 1 0;
  height: auto;
  min-height: 0;
}

/* 覆盖 el-scrollbar 的 max-height prop（内联时高度由 flex 决定） */
.parse-panel.is-inline .log-scroll :deep(.el-scrollbar__wrap) {
  height: 100%;
  max-height: none !important;
}

/* ---------------- 头部 ---------------- */

.panel-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.head-title {
  flex-shrink: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.head-ep {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.head-time {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--accent);
}

/* 圆形重试图标（仅失败态）：尺寸与原来的关闭图标一致 20×20，圆形 */
.head-retry {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: var(--transition-color);
}

.head-retry:hover {
  color: var(--accent);
  background: var(--accent-weak);
}

/* ---------------- 步骤条 ---------------- */

.steps {
  display: flex;
  align-items: flex-start;
  margin: 0;
  padding: var(--space-1) 0;
  list-style: none;
}

.step {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  min-width: 0;
  text-align: center;
}

/* 相邻步骤之间的连接线：从上一步圆心连到当前圆心 */
.step + .step::before {
  content: '';
  position: absolute;
  top: 10px;
  right: 50%;
  left: -50%;
  height: 1px;
  background: var(--border);
}

.step-dot {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-inset);
  color: var(--text-faint);
}

.step-label {
  font-size: 11.5px;
  color: var(--text-muted);
  white-space: nowrap;
}

.step-status {
  font-size: 10.5px;
  color: var(--text-faint);
}

.step.is-active .step-dot {
  border-color: var(--accent);
  background: var(--accent-weak);
  color: var(--accent);
}

.step.is-active .step-label {
  color: var(--text);
}

.step.is-active .step-status {
  color: var(--accent);
}

.step.is-done .step-dot {
  border-color: var(--success);
  background: var(--success-weak);
  color: var(--success);
}

.step.is-done .step-label {
  color: var(--text-regular);
}

.step.is-done .step-status {
  color: var(--success);
}

.step.is-failed .step-dot {
  border-color: var(--danger);
  background: var(--danger-weak);
  color: var(--danger);
}

.step.is-failed .step-label {
  color: var(--text-regular);
}

.step.is-failed .step-status {
  color: var(--danger);
}

/* 跳过 / 等待：统一灰态，明确区别于成功 */
.step.is-skipped .step-label,
.step.is-skipped .step-status,
.step.is-waiting .step-label,
.step.is-waiting .step-status {
  color: var(--text-faint);
}

/* ---------------- 日志 ---------------- */

.log-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--text-muted);
}

.log-count {
  font-family: var(--font-mono);
  font-size: 10.5px;
  color: var(--text-faint);
}

/* 滚动区容器：滚动交给 el-scrollbar（细滚动条 + 圆角裁剪），日志内容横向可滚 */
.log-scroll {
  background: var(--surface-inset);
  border: 0;
  border-radius: var(--radius-md);
}

/* el-scrollbar 的 view 是 shrink-to-fit：强制至少铺满可视宽，行 hover 底色才不会只盖住文字 */
.log-scroll :deep(.el-scrollbar__view) {
  min-width: 100%;
}

.log {
  display: flex;
  flex-direction: column;
  gap: 2px;
  box-sizing: border-box;
  padding: var(--space-1) var(--space-2) var(--space-2);
}

.log-empty {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  margin: 0;
  padding: 2px 0;
  font-size: 11.5px;
  color: var(--text-muted);
}

.log-empty span {
  min-width: 0;
}

.log-row {
  display: flex;
  align-items: center;
  gap: 6px;
  /* 内容比面板宽时横向滚动；min-width 保证短行也能铺满 */
  width: max-content;
  min-width: 100%;
  box-sizing: border-box;
  font-size: 11.5px;
  line-height: 1.5;
}

.log-row:hover {
  background: var(--surface-3);
  border-radius: var(--radius-sm);
}

.log-icon {
  flex-shrink: 0;
  color: var(--text-faint);
}

.log-row.is-start .log-icon {
  color: var(--accent);
}

.log-row.is-ok .log-icon {
  color: var(--success);
}

.log-row.is-fail .log-icon {
  color: var(--danger);
}

.log-row.is-info .log-icon {
  color: var(--info);
}

.log-title {
  flex-shrink: 0;
  white-space: nowrap;
  color: var(--text-regular);
}

.log-row.is-fail .log-title {
  color: var(--danger);
}

.log-detail {
  flex-shrink: 0;
  max-width: none;
  overflow: visible;
  font-family: var(--font-mono);
  font-size: 10.5px;
  white-space: nowrap;
  color: var(--text-faint);
}

/* 右侧固定列（时间 + 复制）：sticky 贴住滚动区右边，像表格的 fixed 列 */
.log-meta {
  position: sticky;
  right: 0;
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  gap: 2px;
  margin-left: auto;
  padding-left: var(--space-2);
  /* 底色与日志区一致，盖住从下面横向滚过的文字；阴影提示下方还有内容 */
  background: var(--surface-2);
  box-shadow: -8px 0 10px -6px rgb(0 0 0 / 26%);
}

.log-row:hover .log-meta {
  background: var(--surface-3);
}

.log-time {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 10.5px;
  font-variant-numeric: tabular-nums;
  color: var(--text-faint);
  transition: color 0.15s ease;
}

.log-row:hover .log-time {
  color: var(--text-muted);
}

.log-copy {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-regular);
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s ease, color 0.15s ease, background-color 0.15s ease;
}

.log-row:hover .log-copy,
.log-copy:focus-visible {
  opacity: 1;
}

.log-copy:hover {
  background: var(--surface-4);
  color: var(--text);
}

/* ---------------- 失败原因 / 操作 ---------------- */

.error-block {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--space-1);
}

.error-text {
  margin: 0;
  padding: var(--space-1) var(--space-2);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  color: var(--danger);
  background: var(--danger-weak);
  border-radius: var(--radius-sm);
  overflow-wrap: anywhere;
}

/* 操作按钮组：在头部紧跟解析总用时（不再固定在面板底部） */
.panel-actions {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--space-1);
}
</style>
