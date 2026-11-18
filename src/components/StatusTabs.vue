<!-- 通用状态筛选 tab：药丸分段样式（下载页 / 历史记录页共用） -->
<script setup lang="ts" generic="T extends string">
defineProps<{
  /** 选项列表（值 + 显示文案；title/muted/badge 为可选增强，供云盘胶囊等场景） */
  items: ReadonlyArray<{ value: T; label: string; title?: string; muted?: boolean; badge?: string }>
  /** 当前选中值 */
  modelValue: T
  /** 无障碍标签（如「下载状态筛选」） */
  ariaLabel?: string
  /** 允许换行（选项较多时使用） */
  wrap?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: T): void
  (e: 'change', value: T): void
}>()

function select(value: T) {
  emit('update:modelValue', value)
  emit('change', value)
}
</script>

<template>
  <div class="status-tabs" :class="{ 'is-wrap': wrap }" role="tablist" :aria-label="ariaLabel">
    <button
      v-for="item in items"
      :key="item.value"
      type="button"
      class="status-tab"
      :class="{ 'is-active': item.value === modelValue, 'is-muted': item.muted }"
      role="tab"
      :aria-selected="item.value === modelValue"
      :title="item.title"
      @click="select(item.value)"
    >
      {{ item.label }}
      <span
        v-if="item.badge"
        class="status-tab-badge"
      >{{ item.badge }}</span>
    </button>
  </div>
</template>

<style scoped>
.status-tabs {
  display: inline-flex;
  padding: var(--space-1);
  border: 1px solid var(--border);
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  transition: var(--transition-color);
}

.status-tabs.is-wrap {
  flex-wrap: wrap;
}

.status-tab {
  padding: 4px 12px;
  border: none;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12.5px;
  line-height: 1.4;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.status-tab:hover {
  color: var(--text);
}

.status-tab.is-active {
  background: var(--accent);
  color: #fff;
}

/* 过期/不可用项：降低不透明度（云盘过期凭据胶囊） */
.status-tab.is-muted {
  opacity: 0.68;
}

.status-tab-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  margin-left: 4px;
  border-radius: 50%;
  background: var(--warning);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
}
</style>
