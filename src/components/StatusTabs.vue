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
/* T1 分段器：卡片托底 + 阴影，选中 = 实心主色胶囊。
   6 处调用（设置页分区 / 点播分类 / 下载 / 历史 / 收藏 / 云盘筛选）共用同一套，不做 variant。 */
.status-tabs {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-card);
  box-shadow: var(--shadow-1);
  transition: box-shadow .18s var(--ease);
}

.status-tabs.is-wrap {
  flex-wrap: wrap;
}

.status-tab {
  height: 32px;
  padding: 0 15px;
  border: none;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12.5px;
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color .16s var(--ease),
    color .16s var(--ease),
    box-shadow .16s var(--ease);
}

.status-tab:hover {
  background: var(--surface-2);
  color: var(--text);
}

.status-tab:focus-visible {
  outline: none;
  box-shadow: var(--ring);
}

.status-tab.is-active {
  background: var(--accent-solid);
  color: var(--accent-on);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, .2), inset 0 1px 0 rgba(255, 255, 255, .14);
}

/* 过期/不可用项：降低不透明度（云盘过期凭据胶囊） */
.status-tab.is-muted {
  opacity: .68;
}

.status-tab-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  margin-left: 5px;
  border-radius: 50%;
  background: var(--warning);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
}
</style>
