<!-- 错误状态：原样展示后端错误文本 + 重试（frontend-design.md §7.2 后端错误文案原则） -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'

const props = defineProps<{
  /** 错误原文（后端字符串原样展示，便于排障） */
  message: string
  /** 标题，默认「加载失败」 */
  title?: string
  /** 重试按钮文案，默认「重试」 */
  retryText?: string
  /** 是否显示重试按钮，默认 true */
  showRetry?: boolean
}>()

const emit = defineEmits<{
  (e: 'retry'): void
}>()

const resolvedTitle = computed(() => props.title || '加载失败')
const resolvedRetryText = computed(() => props.retryText || '重试')
const canRetry = computed(() => props.showRetry !== false)

function handleRetry() {
  emit('retry')
}
</script>

<template>
  <div class="error-state" role="alert">
    <div class="icon-wrap">
      <SolarIcon name="danger-triangle" :size="32" />
    </div>

    <p class="title">{{ resolvedTitle }}</p>

    <!-- 等宽字体展示后端原文，方便直接复制排障（滚动条统一用 el-scrollbar） -->
    <el-scrollbar class="message-scroll" max-height="168px">
      <pre class="message">{{ message }}</pre>
    </el-scrollbar>

    <el-button v-if="canRetry" class="action" type="primary" @click="handleRetry">
      {{ resolvedRetryText }}
    </el-button>
  </div>
</template>

<style scoped>
.error-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  padding: var(--space-6) var(--space-4);
  text-align: center;
}

/* 危险语义图标底托 */
.icon-wrap {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 64px;
  height: 64px;
  margin-bottom: var(--space-1);
  border-radius: var(--radius-pill);
  background: var(--danger-weak);
  color: var(--danger);
}

.title {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 600;
  line-height: 1.5;
}

/* 后端错误原文：等宽字体 + 可滚动，长错误不撑破布局 */
.message-scroll {
  width: 100%;
  max-width: 560px;
}

.message {
  margin: 0;
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface-2);
  color: var(--text-regular);
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.6;
  text-align: left;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.action {
  margin-top: var(--space-2);
}
</style>
