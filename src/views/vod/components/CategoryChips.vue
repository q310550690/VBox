<!-- 分类 chips：内置「首页」+ 后端分类，单选（frontend-design.md §6.1 分类行） -->
<script setup lang="ts">
import { computed } from 'vue'
import type { Category } from '@/types'

const props = defineProps<{
  /** 后端返回的分类列表 */
  categories: Category[]
  /** 当前选中的 type_id，空串表示「首页」 */
  active: string
}>()

const emit = defineEmits<{
  (e: 'select', typeId: string): void
}>()

/** 在最前插入内置「首页」（type_id 为空串），不在页面上重复处理 */
const options = computed<Category[]>(() => [
  { type_id: '', type_name: '首页' },
  ...props.categories,
])

function handleSelect(typeId: string) {
  emit('select', typeId)
}
</script>

<template>
  <div class="category-chips" role="group" aria-label="分类筛选">
    <button
      v-for="category in options"
      :key="category.type_id || '__all__'"
      type="button"
      class="chip"
      :class="{ 'chip--active': category.type_id === active }"
      :aria-pressed="category.type_id === active"
      @click="handleSelect(category.type_id)"
    >
      {{ category.type_name }}
    </button>
  </div>
</template>

<style scoped>
.category-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.chip {
  padding: 7px 14px;
  border: 1px solid transparent;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  color: var(--text-regular);
  font: inherit;
  font-size: 13px;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.chip:hover {
  background: var(--surface-3);
  color: var(--text);
}

/* 选中项：--accent 底 + --text-on-accent 文字 */
.chip--active,
.chip--active:hover {
  background: var(--accent);
  color: var(--text-on-accent);
}
</style>
