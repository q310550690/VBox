<!-- 视频卡片：点播 / 搜索等列表复用（视觉与交互统一在 PosterCard，这里只做数据映射） -->
<script setup lang="ts">
import PosterCard from '@/components/PosterCard.vue'
import type { VideoItem } from '@/types'

const props = defineProps<{
  /** 视频条目 */
  item: VideoItem
  /** 禁用态（如源类型暂不支持时置灰） */
  disabled?: boolean
}>()

const emit = defineEmits<{
  (e: 'open', item: VideoItem): void
}>()
</script>

<template>
  <PosterCard
    :title="props.item.name"
    :cover="props.item.cover"
    :subtitle="props.item.remark ?? ''"
    :description="props.item.siteName ? `来源 · ${props.item.siteName}` : undefined"
    :disabled="props.disabled"
    @open="emit('open', props.item)"
  />
</template>
