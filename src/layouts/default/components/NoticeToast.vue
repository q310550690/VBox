<!-- 全局提示 toast（frontend-design.md §7.1）
     - 数据源：useUiStore().notices（notify() 入队，4s 自动消失，最多同时 3 条）
     - 固定右上角，分级左侧 3px 边框 + 图标；点击卡片或关闭按钮可立即关闭
     - 容器不拦截鼠标（pointer-events:none），单条卡片恢复可点击 -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import { useUiStore } from '@/store/ui'
import type { ToastLevel } from '@/types'

const ui = useUiStore()

/** store 已限制为 3 条，这里再兜底一次，保证布局稳定 */
const notices = computed(() => ui.notices.slice(-3))

/** 级别 → 图标（Solar Linear，见 src/icons/solar.ts） */
const LEVEL_ICONS: Record<ToastLevel, SolarIconName> = {
  info: 'info-circle',
  success: 'check-circle',
  warning: 'danger-triangle',
  error: 'close-circle',
}
</script>

<template>
  <TransitionGroup name="notice" tag="div" class="notice-host" aria-live="polite">
    <div
      v-for="notice in notices"
      :key="notice.id"
      class="notice"
      :class="`is-${notice.level}`"
      @click="ui.dismissNotice(notice.id)"
    >
      <SolarIcon class="notice-icon" :size="16" :name="LEVEL_ICONS[notice.level]" />
      <span class="notice-text">{{ notice.text }}</span>
      <button class="notice-close" type="button" aria-label="关闭提示" @click.stop="ui.dismissNotice(notice.id)">
        <SolarIcon :size="12" name="close" />
      </button>
    </div>
  </TransitionGroup>
</template>

<style scoped>
.notice-host {
  position: fixed;
  top: calc(var(--titlebar-h) + var(--space-3));
  right: var(--space-5);
  z-index: 3000;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  width: 340px;
  max-width: calc(100vw - var(--space-6));
  pointer-events: none;
}

.notice {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: 10px 12px 10px 13px;
  border: 1px solid var(--edge);
  border-left: 3px solid var(--accent);
  border-radius: var(--radius-md);
  background: var(--surface);
  box-shadow: var(--shadow-2);
  cursor: pointer;
  pointer-events: auto;
  transition: var(--transition-color);
}

.notice:hover {
  border-color: var(--accent);
  border-left-color: var(--accent);
}

.notice.is-info {
  border-left-color: var(--accent);
}

.notice.is-success {
  border-left-color: var(--success);
}

.notice.is-warning {
  border-left-color: var(--warning);
}

.notice.is-error {
  border-left-color: var(--danger);
}

.notice:hover.is-success {
  border-color: var(--success);
}

.notice:hover.is-warning {
  border-color: var(--warning);
}

.notice:hover.is-error {
  border-color: var(--danger);
}

.notice-icon {
  flex-shrink: 0;
  margin-top: 2px;
  color: var(--accent);
}

.is-success .notice-icon {
  color: var(--success);
}

.is-warning .notice-icon {
  color: var(--warning);
}

.is-error .notice-icon {
  color: var(--danger);
}

.notice-text {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  line-height: 1.5;
  color: var(--text);
  word-break: break-word;
}

.notice-close {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
  transition: var(--transition-color);
}

.notice-close:hover {
  color: var(--text);
  background: var(--surface-2);
}

/* 淡入淡出 + 高度变化时的平滑位移 */
.notice-enter-active,
.notice-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}

.notice-enter-from,
.notice-leave-to {
  opacity: 0;
  transform: translateX(var(--space-3));
}

.notice-move {
  transition: transform 0.2s ease;
}
</style>
