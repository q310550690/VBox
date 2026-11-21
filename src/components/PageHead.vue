<!-- 页面头部通用骨架：左侧筛选/分类区（自适应剩余宽度）+ 右侧工具胶囊区（不收缩）。
     首页 / 云盘 / 收藏 / 历史 / 下载共用；sticky 默认开启，与内容区顶部留 6px。 -->
<script setup lang="ts">
withDefaults(
  defineProps<{
    /** 是否吸附在内容区顶部 */
    sticky?: boolean
    /** 窄屏时是否允许工具区换行到下一行 */
    wrap?: boolean
  }>(),
  { sticky: true, wrap: false },
)
</script>

<template>
  <header
    class="page-head"
    :class="{ 'is-sticky': sticky, 'is-wrap': wrap }"
  >
    <div class="page-head__lead">
      <slot />
    </div>
    <div
      v-if="$slots.tools"
      class="page-head__tools"
    >
      <slot name="tools" />
    </div>
  </header>
</template>

<style scoped>
.page-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.page-head.is-sticky {
  position: sticky;
  top: 6px;
  z-index: 99;
}

.page-head.is-wrap {
  flex-wrap: wrap;
}

.page-head__lead {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.page-head__tools {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--space-2);
}
</style>
