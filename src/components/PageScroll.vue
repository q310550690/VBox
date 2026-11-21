<!-- 页面级滚动容器：每个页面自己持有 el-scrollbar，
     页面被 KeepAlive 缓存时，滚动位置随页面 DOM 一起保留，不再依赖外壳的共享滚动区。 -->
<script setup lang="ts">
defineOptions({ name: 'PageScroll' })

// fill: 页面内容不足一屏时也撑满滚动视口（页内 flex 才能拿到剩余高度）
withDefaults(defineProps<{ fill?: boolean }>(), { fill: false })
</script>

<template>
  <el-scrollbar
    class="page-scroll"
    :class="{ 'page-scroll--fill': fill }"
  >
    <slot />
  </el-scrollbar>
</template>

<style scoped>
.page-scroll {
  height: 100%;
}

.page-scroll :deep(.el-scrollbar__view) {
  min-height: 100%;
}

/* 给百分比 min-height 一个确定的包含块高度，页面才能真的撑满一屏 */
.page-scroll--fill :deep(.el-scrollbar__view) {
  height: 100%;
}
</style>
