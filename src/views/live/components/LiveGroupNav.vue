<!-- 直播分组导航（frontend-design.md §6.5）：左侧 120px 纵向列表，当前分组高亮 -->
<script setup lang="ts">
import type { LiveGroup } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 直播分组列表（来自 api.liveGroups()） */
    groups: LiveGroup[]
    /** 当前选中的分组 key */
    active: string
    /** 分组加载中：渲染骨架占位 */
    loading?: boolean
  }>(),
  { loading: false },
)

const emit = defineEmits<{ (e: 'select', key: string): void }>()

/** 点击分组：重复点击当前项不重复请求 */
function onSelect(group: LiveGroup) {
  if (group.key === props.active) return
  emit('select', group.key)
}
</script>

<template>
  <nav class="live-group-nav" aria-label="直播分组">
    <el-scrollbar class="group-scroll">
      <div class="group-list">
        <template v-if="loading">
          <span v-for="i in 6" :key="`group-sk-${i}`" class="group-skeleton" aria-hidden="true" />
        </template>
        <template v-else>
          <button
            v-for="group in groups"
            :key="group.key"
            type="button"
            class="group-item"
            :class="{ active: group.key === active }"
            :aria-current="group.key === active ? 'true' : undefined"
            @click="onSelect(group)"
          >
            <span class="group-label ellipsis-1">{{ group.name }}</span>
          </button>
          <p v-if="!groups.length" class="group-empty">暂无分组</p>
        </template>
      </div>
    </el-scrollbar>
  </nav>
</template>

<style scoped>
.live-group-nav {
  flex: 0 0 120px;
  width: 120px;
  min-height: 0;
  padding-right: var(--space-2);
}

.group-scroll {
  height: 100%;
}

.group-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.group-item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: var(--space-2) var(--space-3);
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-regular);
  font-size: 13px;
  text-align: left;
  cursor: pointer;
  transition: var(--transition-color);
}

.group-item:hover {
  background: var(--surface-2);
  color: var(--text);
}

.group-item.active {
  background: var(--accent-weak);
  border-color: transparent;
  color: var(--accent);
  font-weight: 600;
}

.group-label {
  min-width: 0;
}

.group-skeleton {
  height: 34px;
  border-radius: var(--radius-md);
  background: var(--surface-3);
  animation: group-pulse 1.2s ease-in-out infinite;
}

.group-empty {
  margin: 0;
  padding: var(--space-2);
  font-size: 12px;
  color: var(--text-faint);
}

@keyframes group-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.55;
  }
}
</style>
