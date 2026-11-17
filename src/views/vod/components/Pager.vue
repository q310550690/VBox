<!-- 分页器：上一页 / 第 N 页 / 下一页（frontend-design.md §6.1 分页与末页提示） -->
<script setup lang="ts">
import SolarIcon from '@/components/SolarIcon.vue'

defineProps<{
  /** 当前页码，从 1 开始 */
  page: number
  /** 后端是否还有下一页（前端按空数组判定） */
  hasNext: boolean
  /** 是否加载中：加载中禁用翻页，避免重复请求 */
  loading?: boolean
}>()

const emit = defineEmits<{
  (e: 'prev'): void
  (e: 'next'): void
}>()

function handlePrev() {
  emit('prev')
}

function handleNext() {
  emit('next')
}
</script>

<template>
  <nav class="pager" aria-label="分页导航">
    <el-button :disabled="page <= 1 || !!loading" @click="handlePrev">
      <template #icon>
        <SolarIcon name="arrow-left" size="1em" />
      </template>
      <span>上一页</span>
    </el-button>

    <span class="page-indicator">第 {{ page }} 页</span>

    <el-button :disabled="!hasNext || !!loading" @click="handleNext">
      <span>下一页</span>
      <SolarIcon class="pager-icon-right" name="arrow-right" size="1em" />
    </el-button>

    <!-- 无末页判定：后端返回空数组后前端置灰并提示 -->
    <span v-if="!hasNext" class="end-hint">已到最后一页</span>
  </nav>
</template>

<style scoped>
.pager {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: var(--space-3);
  padding: var(--space-4) 0;
}

.page-indicator {
  min-width: 72px;
  color: var(--text-regular);
  font-size: 13px;
  text-align: center;
}

/* 下一页图标在文字右侧，补齐与文字的间距 */
.pager-icon-right {
  margin-left: var(--space-1);
}

.end-hint {
  color: var(--text-muted);
  font-size: 12px;
}
</style>
