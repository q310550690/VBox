<!-- 整页模糊封面背景（点播搜索页 / 播放详情页共用）
     视觉参数对齐首页轮播 HomeCarousel 的 .hero-bg-img：blur(46px) saturate(1.25) scale(1.06)，
     区别是本层不裁径向遮罩，直接铺满整个内容区；无封面时不渲染图片，露出原页面底色。 -->
<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  /** 封面地址；为空（无封面 / 加载前）时只保留透明层，页面底色照常显示 */
  cover?: string | null
}>()

/** 背景图样式：url 里的引号需要转义，避免封面地址破坏 style 属性 */
const imageStyle = computed(() => {
  const src = (props.cover ?? '').trim()
  if (!src) return null
  return { backgroundImage: `url("${src.replace(/"/g, '\\"')}")` }
})
</script>

<template>
  <div class="blur-backdrop" aria-hidden="true">
    <Transition name="blur-backdrop-fade">
      <div v-if="imageStyle" :key="cover ?? ''" class="blur-backdrop-img" :style="imageStyle" />
    </Transition>
  </div>
</template>

<style scoped>
.blur-backdrop {
  position: absolute;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  pointer-events: none;
}

.blur-backdrop-img {
  position: absolute;
  inset: 0;
  background-position: center;
  background-size: cover;
  /* 整页版本比首页轮播略强（轮播有径向遮罩+文字压暗，这里铺满全页要靠透明度撑住存在感） */
  opacity: var(--blur-backdrop-opacity);
  filter: blur(46px) saturate(1.25) brightness(var(--blur-backdrop-brightness, 1));
  transform: scale(1.06);
  will-change: opacity;
}

.blur-backdrop-fade-enter-active,
.blur-backdrop-fade-leave-active {
  transition: opacity 0.8s ease;
}

.blur-backdrop-fade-enter-from,
.blur-backdrop-fade-leave-to {
  opacity: 0;
}
</style>
