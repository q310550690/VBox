<!-- 观看历史卡片：统一海报卡（PosterCard）+ 进度角标 / 封面进度条 + 播放·删除操作
     点击卡片继续观看 -->
<script setup lang="ts">
import { computed } from 'vue'
import PosterCard from '@/components/PosterCard.vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { HistoryItem } from '@/types'
import { formatDuration, formatRelativeTime } from '@/utils/format'

const props = defineProps<{
  item: HistoryItem
}>()

const emit = defineEmits<{
  (e: 'play', item: HistoryItem): void
  (e: 'remove', item: HistoryItem): void
}>()

/** 旧数据可能缺站点 key / 影片 id：这类记录无法回跳详情页，卡片上直接标注。 */
const invalidSource = computed(
  () => !(props.item.siteKey || '').trim() || !(props.item.id || '').trim(),
)

/** 观看进度百分比 */
const percent = computed(() => {
  const { positionSec, durationSec } = props.item
  if (!durationSec || durationSec <= 0) return 0
  return Math.max(0, Math.min(100, Math.round((positionSec / durationSec) * 100)))
})

/** 站点 · 第 N 集（来源失效时明确提示，而不是显示成一条正常的空记录） */
const meta = computed(() => {
  if (invalidSource.value) return '来源已失效'
  const ep = props.item.epTitle || `第${props.item.epIndex + 1}集`
  return [props.item.siteName, ep].filter(Boolean).join(' · ')
})

/** 相对时间 + 已看时长 */
const subline = computed(
  () => `${formatRelativeTime(props.item.updatedAt)} · 已看 ${formatDuration(props.item.positionSec)}`,
)

/** 右上角进度角标（0% 不显示） */
const badge = computed(() =>
  percent.value > 0 ? { text: `${percent.value}%`, tone: 'progress' as const } : null,
)

const ariaLabel = computed(
  () => `${props.item.name}，${meta.value}，已看 ${percent.value}%，点击继续观看`,
)
</script>

<template>
  <PosterCard
    :title="item.name"
    :cover="item.cover"
    :badge="badge"
    :progress="percent"
    :subtitle="meta || undefined"
    :description="subline"
    :aria-label="ariaLabel"
    :hint="invalidSource ? '来源已失效，无法继续播放' : '点击继续观看'"
    @open="emit('play', item)"
  >
    <template #actions>
      <button
        type="button"
        class="action-btn"
        :aria-label="`继续观看 ${item.name}`"
        @click.stop="emit('play', item)"
      >
        <SolarIcon name="play" :size="15" />
      </button>
      <button
        type="button"
        class="action-btn"
        :aria-label="`删除 ${item.name} 的观看记录`"
        @click.stop="emit('remove', item)"
      >
        <SolarIcon name="trash-bin-minimalistic" :size="15" />
      </button>
    </template>
  </PosterCard>
</template>
