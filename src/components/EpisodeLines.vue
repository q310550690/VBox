<!-- 线路选择：同一资源有多条播放线路时的 chips（frontend-design.md §6.2 线路行）
     线路始终一行：溢出时横向滚动（滚动条隐藏），两端按需浮出箭头点击翻页（同首页状态 tab） -->
<script setup lang="ts">
import { computed, nextTick, onActivated, onMounted, onUnmounted, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import { buildLineLabels } from '@/utils/episodeLines'
import type { EpisodeGroup } from '@/types'

const props = defineProps<{
  /** 全部线路分组（每个 group 即一条线路） */
  groups: EpisodeGroup[]
  /** 当前线路索引 */
  activeLine: number
}>()

const emit = defineEmits<{
  (e: 'select', index: number): void
}>()

/** 线路名：后端可能没有 group 名，用序号兜底；同名线路追加 -2，和下载目录保持一致 */
const lineLabels = computed(() => buildLineLabels(props.groups))

function handleSelect(index: number) {
  emit('select', index)
}

const viewport = ref<HTMLElement | null>(null)
const canScrollLeft = ref(false)
const canScrollRight = ref(false)
let resizeObserver: ResizeObserver | null = null

function updateArrows() {
  const el = viewport.value
  if (!el) {
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
  if (!el || el.scrollWidth <= el.clientWidth + 1) return
  event.preventDefault()
  el.scrollLeft += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY
}

watch(() => props.groups, () => void nextTick(updateArrows))

onMounted(() => {
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
  <!-- 不足两条线路时无需切换，整块隐藏 -->
  <div v-if="groups.length > 1" class="episode-lines">
    <span class="label">线路</span>
    <div class="chips-wrap" @wheel="onWheel">
      <div
        ref="viewport"
        class="chips"
        role="group"
        aria-label="播放线路"
        @scroll.passive="updateArrows"
      >
        <button
          v-for="(group, index) in groups"
          :key="`${index}-${group.group}`"
          type="button"
          class="line-chip"
          :class="{ 'line-chip--active': index === activeLine }"
          :aria-pressed="index === activeLine"
          @click="handleSelect(index)"
        >
          {{ lineLabels[index] }}
        </button>
      </div>
      <el-tooltip
        content="向前滚动"
        placement="bottom"
        :show-after="300"
        :disabled="!canScrollLeft"
      >
        <el-button
          class="chips-arrow is-left"
          :class="{ 'is-hidden': !canScrollLeft }"
          circle
          :aria-hidden="!canScrollLeft"
          :tabindex="canScrollLeft ? 0 : -1"
          aria-label="向前滚动"
          @click="scrollByStep(-1)"
        >
          <SolarIcon name="alt-arrow-left" :size="14" />
        </el-button>
      </el-tooltip>
      <el-tooltip
        content="向后滚动"
        placement="bottom"
        :show-after="300"
        :disabled="!canScrollRight"
      >
        <el-button
          class="chips-arrow is-right"
          :class="{ 'is-hidden': !canScrollRight }"
          circle
          :aria-hidden="!canScrollRight"
          :tabindex="canScrollRight ? 0 : -1"
          aria-label="向后滚动"
          @click="scrollByStep(1)"
        >
          <SolarIcon name="alt-arrow-right" :size="14" />
        </el-button>
      </el-tooltip>
    </div>
  </div>
</template>

<style scoped>
.episode-lines {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  min-width: 0;
}

.label {
  flex-shrink: 0;
  padding: 7px 0;
  color: var(--text-muted);
  font-size: 13px;
  line-height: 1;
}

/* 单行视口：溢出横向滚动、滚动条隐藏；箭头浮在两端（同首页状态 tab） */
.chips-wrap {
  position: relative;
  flex: 1;
  min-width: 0;
}

.chips {
  display: flex;
  flex-wrap: nowrap;
  gap: var(--space-2);
  min-width: 0;
  overflow-x: auto;
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
}

.chips::-webkit-scrollbar {
  display: none;
}

.chips-arrow {
  position: absolute;
  top: 50%;
  z-index: 2;
  margin: 0;
  transform: translateY(-50%);
  transition:
    opacity 0.16s var(--ease),
    transform 0.16s var(--ease);
}

.chips-arrow.is-left {
  left: 0;
}

.chips-arrow.is-right {
  right: 0;
}

.chips-arrow.is-hidden {
  opacity: 0;
  transform: translateY(-50%) scale(0.9);
  pointer-events: none;
}

.line-chip {
  padding: 7px 10px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-inset);
  color: var(--text-regular);
  font: inherit;
  font-size: 12.5px;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.line-chip:hover {
  background: color-mix(in srgb, var(--el-color-primary) 10%, var(--surface-inset));
  color: var(--text);
}

/* 当前线路：主色染底 + 提亮文字，不实心（实心只给"正在播放"和主按钮） */
.line-chip--active,
.line-chip--active:hover {
  background: color-mix(in srgb, var(--el-color-primary) 16%, var(--surface-card));
  color: var(--text-strong);
  font-weight: 600;
}
</style>
