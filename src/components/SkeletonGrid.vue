<!-- 骨架屏：列表/网格加载态占位（frontend-design.md §6 通用四态「加载」，亮暗主题均可见） -->
<script setup lang="ts">
/**
 * SkeletonGrid：加载态占位骨架。
 * - variant='card'：视频卡片网格骨架（封面 2:3 + 两行文字），栅格与 VideoGrid 保持一致；
 * - variant='row'：行列表骨架（缩略图 + 三行文字），用于历史/下载等列表页。
 * 颜色只使用 --surface-2 / --surface-3 两个 token + 脉冲动画，禁止写死色值。
 */
withDefaults(
  defineProps<{
    /** 骨架块数量，默认 12 */
    count?: number
    /** 骨架形态：卡片网格 / 行列表 */
    variant?: 'card' | 'row'
  }>(),
  { count: 12, variant: 'card' },
)
</script>

<template>
  <div
    class="skeleton-grid"
    :class="variant === 'row' ? 'skeleton-grid--row' : 'skeleton-grid--cards'"
    role="status"
    aria-label="内容加载中"
  >
    <!-- 行列表骨架 -->
    <template v-if="variant === 'row'">
      <div v-for="i in count" :key="`row-${i}`" class="sk-row">
        <div class="sk-block sk-thumb"></div>
        <div class="sk-row-body">
          <div class="sk-block sk-line sk-line--title"></div>
          <div class="sk-block sk-line"></div>
          <div class="sk-block sk-line sk-line--short"></div>
        </div>
      </div>
    </template>

    <!-- 卡片网格骨架 -->
    <template v-else>
      <div v-for="i in count" :key="`card-${i}`" class="sk-card">
        <div class="sk-block sk-cover"></div>
        <div class="sk-block sk-line sk-line--title"></div>
        <div class="sk-block sk-line sk-line--remark"></div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.skeleton-grid {
  width: 100%;
}

/* 卡片形态：栅格与 VideoGrid 完全一致 */
.skeleton-grid--cards {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: var(--space-3);
}

/* 行形态：竖向列表 */
.skeleton-grid--row {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

/* 卡片骨架：与 PosterCard 一致的卡片外壳（白底 + 圆角 + 描边 + 内边距） */
.sk-card {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
  padding: var(--space-2);
  border: 0;
  border-radius: var(--radius-lg);
  background: var(--surface);
}

/* 封面骨架块 2:3，与 PosterCard 的封面尺寸一致（双类名提高优先级，避免被 .sk-block 的圆角覆盖） */
.sk-block.sk-cover {
  width: 100%;
  aspect-ratio: 2 / 3;
  border-radius: var(--radius-md);
}

.sk-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface);
}

.sk-thumb {
  width: 48px;
  aspect-ratio: 2 / 3;
  flex-shrink: 0;
  border-radius: var(--radius-sm);
}

.sk-row-body {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
}

/* 骨架块：--surface-2/--surface-3 脉冲 + 描边，亮色主题下也能看清块形状 */
.sk-block {
  border: 0;
  border-radius: var(--radius-sm);
  background-color: var(--surface-3);
  animation: vbox-skeleton-pulse 1.4s ease-in-out infinite;
}

.sk-line {
  height: 12px;
}

.sk-line--title {
  width: 82%;
  height: 14px;
}

.sk-line--remark,
.sk-line--short {
  width: 48%;
}

@keyframes vbox-skeleton-pulse {
  0%,
  100% {
    background-color: var(--surface-3);
    opacity: 1;
  }
  50% {
    background-color: var(--surface-2);
    opacity: 0.72;
  }
}

/* 减少动态效果偏好：保留静态骨架，关闭脉冲 */
@media (prefers-reduced-motion: reduce) {
  .sk-block {
    animation: none;
    background-color: var(--surface-3);
    opacity: 1;
  }
}
</style>
