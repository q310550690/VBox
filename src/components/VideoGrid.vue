<!-- 视频网格：150px 自适应列 + VideoCard / SkeletonGrid（frontend-design.md §6.1） -->
<script setup lang="ts">
import VideoCard from './VideoCard.vue'
import SkeletonGrid from './SkeletonGrid.vue'
import type { VideoItem } from '@/types'

withDefaults(
  defineProps<{
    /** 视频条目列表 */
    items: VideoItem[]
    /** 是否加载中：为 true 时渲染骨架屏 */
    loading?: boolean
    /** 加载态骨架数量，默认 12 */
    skeletonCount?: number
  }>(),
  { loading: false, skeletonCount: 12 },
)

const emit = defineEmits<{
  (e: 'select', item: VideoItem): void
}>()

function handleOpen(item: VideoItem) {
  emit('select', item)
}
</script>

<template>
  <div class="video-grid-host">
    <!-- 加载态：骨架屏（不使用居中文字） -->
    <SkeletonGrid v-if="loading" :count="skeletonCount" variant="card" />

    <!-- 有数据：自适应网格 -->
    <div v-else-if="items.length > 0" class="video-grid">
      <VideoCard v-for="item in items" :key="item.id" :item="item" @open="handleOpen" />
    </div>

    <!-- 空数组：仅保留空容器，空态文案与操作由页面负责 -->
    <div v-else class="video-grid video-grid--empty"></div>
  </div>
</template>

<style scoped>
.video-grid-host {
  width: 100%;
}

.video-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: var(--space-3);
}

/* 空容器不占位、不显示任何文案 */
.video-grid--empty {
  display: block;
}
</style>
