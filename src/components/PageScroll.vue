<!-- 页面级滚动容器：每个页面自己持有 el-scrollbar，
     页面被 KeepAlive 缓存时，滚动位置随页面 DOM 一起保留，不再依赖外壳的共享滚动区。 -->
<script setup lang="ts">
defineOptions({ name: 'PageScroll' })
</script>

<template>
  <el-scrollbar class="page-scroll">
    <slot />
  </el-scrollbar>
</template>

<style scoped>
.page-scroll {
  height: 100%;
}
</style>
