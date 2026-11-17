<!-- 线路选择：同一资源有多条播放线路时的 chips（frontend-design.md §6.2 线路行） -->
<script setup lang="ts">
import { computed } from 'vue'
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
</script>

<template>
  <!-- 不足两条线路时无需切换，整块隐藏 -->
  <div v-if="groups.length > 1" class="episode-lines">
    <span class="label">线路</span>
    <div class="chips" role="group" aria-label="播放线路">
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

.chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  min-width: 0;
}

.line-chip {
  padding: 7px 14px;
  border: 1px solid transparent;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  color: var(--text-regular);
  font: inherit;
  font-size: 13px;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.line-chip:hover {
  background: var(--surface-3);
  color: var(--text);
}

/* 当前线路：--accent 高亮 */
.line-chip--active,
.line-chip--active:hover {
  background: var(--accent);
  color: var(--text-on-accent);
}
</style>
