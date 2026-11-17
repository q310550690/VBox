<!-- 豆瓣卡片：统一海报卡（PosterCard）+ 评分角标；标题 / 年份地区 / 简介各占一行 -->
<script setup lang="ts">
import { computed } from 'vue'
import PosterCard from '@/components/PosterCard.vue'
import { proxyImage } from '@/utils/image'
import type { DoubanItem } from '@/types'

const props = withDefaults(
  defineProps<{
    item: DoubanItem
    /** 本机图片代理前缀（豆瓣封面需 Referer） */
    imageProxy?: string | null
  }>(),
  { imageProxy: null },
)

const emit = defineEmits<{
  (e: 'select', item: DoubanItem): void
}>()

const cover = computed(() => proxyImage(props.item.cover, props.imageProxy))

/** 评分（无评分不显示角标） */
const rating = computed(() => {
  const value = props.item.rating
  return typeof value === 'number' && value > 0 ? value.toFixed(1) : ''
})

const badge = computed(() =>
  rating.value ? { text: rating.value, tone: 'rating' as const } : null,
)

/** info（地区 / 类型 / 导演 / 主演）拆段 */
const infoParts = computed<string[]>(() =>
  (props.item.info ?? '')
    .split(' / ')
    .map((part) => part.trim())
    .filter(Boolean),
)

/** 年份 + 地区：单独一行 */
const yearArea = computed(() => {
  const region = infoParts.value[0] ?? ''
  return [props.item.year, region].filter(Boolean).join(' · ')
})

/** 介绍：优先用简介，否则用类型 / 导演 / 主演 */
const desc = computed(() => {
  const intro = (props.item.intro ?? '').trim()
  if (intro) return intro
  return infoParts.value.slice(1).join(' / ')
})
</script>

<template>
  <PosterCard
    :title="item.title"
    :cover="cover"
    :badge="badge"
    :subtitle="yearArea || undefined"
    :description="desc || undefined"
    @open="emit('select', item)"
  />
</template>
