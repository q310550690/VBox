<!-- 通用状态筛选 tab：药丸分段样式（下载页 / 历史记录页共用）
     scrollable 时在胶囊内部左右两侧各出一枚箭头，内容横向滚动，滚到边界即收起 -->
<script setup lang="ts" generic="T extends string">
import { nextTick, onActivated, onMounted, onUnmounted, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'

const props = defineProps<{
  /** 选项列表（值 + 显示文案；title/muted/badge 为可选增强，供云盘胶囊等场景） */
  items: ReadonlyArray<{ value: T; label: string; title?: string; muted?: boolean; badge?: string }>
  /** 当前选中值 */
  modelValue: T
  /** 无障碍标签（如「下载状态筛选」） */
  ariaLabel?: string
  /** 允许换行（选项较多时使用） */
  wrap?: boolean
  /** 内容溢出时在胶囊内横向滚动，两端按需显示箭头 */
  scrollable?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: T): void
  (e: 'change', value: T): void
}>()

const viewport = ref<HTMLElement | null>(null)
const canScrollLeft = ref(false)
const canScrollRight = ref(false)
let resizeObserver: ResizeObserver | null = null

function updateArrows() {
  const el = viewport.value
  if (!el || !props.scrollable) {
    canScrollLeft.value = false
    canScrollRight.value = false
    return
  }
  canScrollLeft.value = el.scrollLeft > 1
  canScrollRight.value = el.scrollLeft + el.clientWidth < el.scrollWidth - 1
}

/** 箭头一次滚 70% 视口（至少 160px），平滑滚动 */
function scrollByStep(direction: -1 | 1) {
  const el = viewport.value
  if (!el) return
  const step = Math.max(160, Math.round(el.clientWidth * 0.7))
  el.scrollBy({ left: direction * step, behavior: 'smooth' })
}

/** 纵向滚轮 → 横向滚动；未溢出时不拦截，滚轮照常滚页面 */
function onWheel(event: WheelEvent) {
  const el = viewport.value
  if (!props.scrollable || !el || el.scrollWidth <= el.clientWidth + 1) return
  event.preventDefault()
  el.scrollLeft += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY
}

function select(value: T) {
  emit('update:modelValue', value)
  emit('change', value)
}

watch(
  () => props.items,
  () => void nextTick(updateArrows),
)

onMounted(() => {
  if (!props.scrollable) return
  const el = viewport.value
  if (!el) return
  resizeObserver = new ResizeObserver(updateArrows)
  resizeObserver.observe(el)
  void nextTick(updateArrows)
})

onActivated(() => void nextTick(updateArrows))

onUnmounted(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})

defineExpose({ updateArrows })
</script>

<template>
  <div
    class="status-tabs"
    :class="{ 'is-scrollable': scrollable }"
    @wheel="onWheel"
  >
    <el-tooltip
      v-if="scrollable"
      content="向前滚动"
      placement="bottom"
      :show-after="300"
      :disabled="!canScrollLeft"
    >
      <el-button
        class="tabs-arrow is-left"
        :class="{ 'is-hidden': !canScrollLeft }"
        circle
        :aria-hidden="!canScrollLeft"
        :tabindex="canScrollLeft ? 0 : -1"
        aria-label="向前滚动"
        @click="scrollByStep(-1)"
      >
        <SolarIcon name="alt-arrow-left" :size="16" />
      </el-button>
    </el-tooltip>

    <div
      ref="viewport"
      class="tabs-viewport"
      :class="{ 'is-wrap': wrap }"
      role="tablist"
      :aria-label="ariaLabel"
      @scroll.passive="updateArrows"
    >
      <button
        v-for="item in items"
        :key="item.value"
        type="button"
        class="status-tab"
        :class="{ 'is-active': item.value === modelValue, 'is-muted': item.muted }"
        role="tab"
        :aria-selected="item.value === modelValue"
        :title="item.title"
        @click="select(item.value)"
      >
        {{ item.label }}
        <span
          v-if="item.badge"
          class="status-tab-badge"
        >{{ item.badge }}</span>
      </button>
    </div>

    <el-tooltip
      v-if="scrollable"
      content="向后滚动"
      placement="bottom"
      :show-after="300"
      :disabled="!canScrollRight"
    >
      <el-button
        class="tabs-arrow is-right"
        :class="{ 'is-hidden': !canScrollRight }"
        circle
        :aria-hidden="!canScrollRight"
        :tabindex="canScrollRight ? 0 : -1"
        aria-label="向后滚动"
        @click="scrollByStep(1)"
      >
        <SolarIcon name="alt-arrow-right" :size="16" />
      </el-button>
    </el-tooltip>
  </div>
</template>

<style scoped>
/* T1 分段器：卡片托底 + 阴影，选中 = 实心主色胶囊。
   6 处调用（设置页分区 / 点播分类 / 下载 / 历史 / 收藏 / 云盘筛选）共用同一套，不做 variant。 */
.status-tabs {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 4px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-card);
  box-shadow: var(--shadow-1);
  transition: box-shadow .18s var(--ease);
}

/* 可滚动：胶囊自己撑满调用方给的最大宽度，内部视口吃掉剩余空间 */
.status-tabs.is-scrollable {
  position: relative;
  display: flex;
  max-width: 100%;
  min-width: 0;
}

.tabs-viewport {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}

.tabs-viewport.is-wrap {
  flex-wrap: wrap;
}

.status-tabs.is-scrollable .tabs-viewport {
  flex: 1 1 auto;
  overflow-x: auto;
  overflow-y: hidden;
  overscroll-behavior-x: contain;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.status-tabs.is-scrollable .tabs-viewport::-webkit-scrollbar {
  display: none;
}

.status-tabs.is-scrollable .status-tab {
  flex: none;
}

/* 边界箭头：浮在标签之上的圆钮（不占位、不挤压标签），滚到尽头淡出收起；
   ::before 是同底色渐隐罩，让被压住的标签自然淡出而不是硬切。
   按钮本体复用 el-button circle（软底 + 40px 命中区），这里只管定位与显隐。 */
.tabs-arrow {
  position: absolute;
  top: 50%;
  z-index: 2;
  margin: 0;
  transform: translateY(-50%);
  transition:
    opacity .16s var(--ease),
    transform .16s var(--ease),
    background-color .16s var(--ease),
    color .16s var(--ease);
}

.tabs-arrow::before {
  content: '';
  position: absolute;
  top: -4px;
  bottom: -4px;
  z-index: -1;
  width: 44px;
  pointer-events: none;
}

.tabs-arrow.is-left {
  left: 4px;
}

/* .tabs-arrow.is-left::before {
  left: -4px;
  background: linear-gradient(90deg, var(--surface-card) 38%, transparent);
} */

.tabs-arrow.is-right {
  right: 4px;
}

/* .tabs-arrow.is-right::before {
  right: -4px;
  background: linear-gradient(270deg, var(--surface-card) 38%, transparent);
} */

.tabs-arrow.is-hidden {
  opacity: 0;
  transform: translateY(-50%) scale(.9);
  pointer-events: none;
}

.status-tab {
  height: 32px;
  padding: 0 15px;
  border: none;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12.5px;
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color .16s var(--ease),
    color .16s var(--ease),
    box-shadow .16s var(--ease);
}

.status-tab:hover {
  background: var(--surface-2);
  color: var(--text);
}

.status-tab:focus-visible {
  outline: none;
  box-shadow: var(--ring);
}

.status-tab.is-active {
  background: var(--accent-solid);
  color: var(--accent-on);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, .2), inset 0 1px 0 rgba(255, 255, 255, .14);
}

/* 过期/不可用项：降低不透明度（云盘过期凭据胶囊） */
.status-tab.is-muted {
  opacity: .68;
}

.status-tab-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  margin-left: 5px;
  border-radius: 50%;
  background: var(--warning);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
}
</style>
