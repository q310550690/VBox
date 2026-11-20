<!-- 通用提示条：统一 padding/圆角/左侧色条；variant 决定语义色（info / warning / error / success） -->
<script setup lang="ts">
withDefaults(
  defineProps<{
    /** 语义类型：info（默认）/ warning / error / success */
    variant?: 'info' | 'warning' | 'error' | 'success'
  }>(),
  { variant: 'info' },
)
</script>

<template>
  <div class="hint-bar" :class="`is-${variant}`">
    <slot />
  </div>
</template>

<style scoped>
.hint-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  padding: 9px 12px;
  border-left: 3px solid var(--info);
  border-radius: var(--radius-md);
  background: var(--info-weak);
  color: var(--text-regular);
  font-size: 12px;
  line-height: 1.6;
}

.hint-bar.is-warning {
  border-left-color: var(--warning);
  background: var(--warning-weak);
}

.hint-bar.is-error {
  border-left-color: var(--danger);
  background: var(--danger-weak);
  color: var(--danger);
}

.hint-bar.is-success {
  border-left-color: var(--success);
  background: var(--success-weak);
}
</style>
