<!-- 选集网格：当前集高亮并自动滚入可视区（frontend-design.md §6.2 选集区） -->
<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from 'vue'
import type { Episode } from '@/types'

const props = defineProps<{
  /** 当前线路的剧集列表 */
  items: Episode[]
  /** 当前集索引；未选中时为空 */
  activeEp?: number
}>()

const emit = defineEmits<{
  (e: 'play', ep: Episode, index: number): void
}>()

/** 网格根节点：用于定位当前集按钮 */
const gridRef = ref<HTMLElement | null>(null)

/** 集标题：后端偶发空标题，用序号兜底 */
function episodeLabel(episode: Episode, index: number) {
  return episode.title || `第${index + 1}集`
}

function handlePlay(episode: Episode, index: number) {
  emit('play', episode, index)
}

/** 当前集滚动到可视区；jsdom / 非必要环境不支持时静默跳过 */
function scrollActiveIntoView() {
  const active = props.activeEp
  if (active === undefined || active < 0) return
  const root = gridRef.value
  if (!root) return
  const target = root.querySelectorAll<HTMLElement>('.ep-chip')[active]
  if (!target || typeof target.scrollIntoView !== 'function') return
  try {
    target.scrollIntoView({ block: 'nearest' })
  } catch {
    // 单测等无布局环境（jsdom 未实现 scrollIntoView）下忽略即可
  }
}

/** 当前集或集列表变化时，等 DOM 更新后再滚动 */
watch([() => props.activeEp, () => props.items.length], () => {
  nextTick(scrollActiveIntoView)
})

onMounted(() => {
  nextTick(scrollActiveIntoView)
})
</script>

<template>
  <div ref="gridRef" class="episode-grid" role="group" aria-label="选集">
    <button
      v-for="(episode, index) in items"
      :key="index"
      type="button"
      class="ep-chip"
      :class="{ 'ep-chip--active': index === activeEp }"
      :aria-pressed="index === activeEp"
      :title="episode.title || undefined"
      @click="handlePlay(episode, index)"
    >
      {{ episodeLabel(episode, index) }}
    </button>
  </div>
</template>

<style scoped>
.episode-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(72px, auto));
  gap: var(--space-2);
}

/* 集数按钮：填充式（凹槽底、零描边），与输入框/次级按钮同一套语言 */
.ep-chip {
  min-width: 0;
  height: 32px;
  padding: 0 var(--space-2);
  overflow: hidden;
  border: 0;
  border-radius: var(--radius-sm);
  background: var(--surface-inset);
  color: var(--text-regular);
  font: inherit;
  font-size: 13px;
  line-height: 1;
  white-space: nowrap;
  text-overflow: ellipsis;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.ep-chip:hover {
  background: color-mix(in srgb, var(--el-color-primary) 10%, var(--surface-inset));
  color: var(--text);
}

/* 当前集：--accent 底 + --text-on-accent 文字 */
.ep-chip--active,
.ep-chip--active:hover {
  background: var(--accent);
  color: var(--text-on-accent);
  font-weight: 500;
}
</style>
