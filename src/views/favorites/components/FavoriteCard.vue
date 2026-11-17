<!-- 收藏卡片：统一海报卡（PosterCard）+ 查看详情 / 取消收藏操作
     点击卡片查看详情 -->
<script setup lang="ts">
import { computed } from 'vue'
import PosterCard from '@/components/PosterCard.vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { FavoriteItem } from '@/types'

const props = defineProps<{
  item: FavoriteItem
}>()

const emit = defineEmits<{
  (e: 'open', item: FavoriteItem): void
  (e: 'remove', item: FavoriteItem): void
}>()

/** 站点名 · 年份 · 地区（缺省项不展示） */
const meta = computed(() => {
  const parts = [props.item.siteName, props.item.year, props.item.area]
  return parts.filter((v): v is string => Boolean(v && String(v).trim())).join(' · ')
})

const ariaLabel = computed(
  () => `${props.item.name}${meta.value ? `，${meta.value}` : ''}，点击查看详情`,
)
</script>

<template>
  <PosterCard
    :title="item.name"
    :cover="item.cover"
    :subtitle="meta || undefined"
    :description="item.remark || undefined"
    :aria-label="ariaLabel"
    hint="点击查看详情"
    @open="emit('open', item)"
  >
    <template #actions>
      <button
        type="button"
        class="action-btn"
        :aria-label="`查看 ${item.name} 的详情`"
        @click.stop="emit('open', item)"
      >
        <SolarIcon name="play" :size="15" />
      </button>
      <button
        type="button"
        class="action-btn"
        :aria-label="`取消收藏 ${item.name}`"
        @click.stop="emit('remove', item)"
      >
        <SolarIcon name="trash-bin-minimalistic" :size="15" />
      </button>
    </template>
  </PosterCard>
</template>
