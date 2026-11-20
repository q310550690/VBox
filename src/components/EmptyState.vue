<!-- 空状态：无数据时的居中图标 + 标题 + 描述 + 主操作（frontend-design.md §7.2 空态文案原则） -->
<script setup lang="ts">
import SolarIcon from '@/components/SolarIcon.vue'

defineProps<{
  /** 主标题，如「该分类暂无内容」 */
  title: string
  /** 补充说明 */
  desc?: string
  /** 主操作按钮文案；不传则不显示按钮 */
  actionText?: string
}>()

const emit = defineEmits<{
  (e: 'action'): void
}>()

function handleAction() {
  emit('action')
}
</script>

<template>
  <div class="empty-state" role="status">
    <!-- 图标槽：#icon 可替换，默认使用 Solar Linear 图标（不使用 el-empty，保证主题一致） -->
    <div class="icon-wrap">
      <slot name="icon">
        <SolarIcon name="box" :size="34" />
      </slot>
    </div>

    <p class="title">{{ title }}</p>
    <p v-if="desc" class="desc">{{ desc }}</p>

    <el-button v-if="actionText" class="action" type="primary" @click="handleAction">
      {{ actionText }}
    </el-button>
  </div>
</template>

<style scoped>
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  padding: var(--space-6) var(--space-4);
  text-align: center;
}

/* 图标底托：圆形 --surface-2 底，主题切换时自动跟随 */
.icon-wrap {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  margin-bottom: var(--space-1);
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  color: var(--text-muted);
}

.title {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 600;
  line-height: 1.5;
}

.desc {
  max-width: 420px;
  margin: 0;
  color: var(--text-muted);
  font-size: 13px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.action {
  margin-top: var(--space-2);
}
</style>
