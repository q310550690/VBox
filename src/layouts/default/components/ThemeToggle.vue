<!-- 侧栏底部主题切换按钮（frontend-design.md §4.1）
     - 三态循环：亮色 → 暗色 → 跟随系统（循环动作由父组件调用 ui.cycleMode()）
     - 图标/文案随 mode 变化；折叠（窗口 < 960px）时只显示图标，悬停有 tooltip
     - 图标统一来自 src/icons/solar.ts（Solar Linear），颜色只用 tokens.css 变量 -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import type { ThemeMode } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 当前主题模式：跟随系统 / 亮色 / 暗色 */
    mode: ThemeMode
    /** 实际解析出的明暗（跟随系统时由系统偏好决定） */
    resolved: 'light' | 'dark'
    /** 侧栏是否折叠（折叠时仅图标） */
    collapsed?: boolean
  }>(),
  { collapsed: false },
)

const emit = defineEmits<{ (e: 'toggle'): void }>()

/** 模式对应的中文名（设置页与侧栏共用同一套文案） */
const modeLabel = computed<string>(() => {
  if (props.mode === 'system') return '跟随系统'
  return props.mode === 'dark' ? '暗色' : '亮色'
})

/** 图标：亮色 sun / 暗色 moon / 跟随系统 monitor（Solar Linear） */
const modeIcon = computed<SolarIconName>(() => {
  if (props.mode === 'system') return 'monitor'
  return props.mode === 'dark' ? 'moon' : 'sun'
})

const ariaLabel = computed<string>(() => `切换主题（当前：${modeLabel.value}）`)
</script>

<template>
  <button
    class="theme-toggle"
    :class="{ 'is-collapsed': collapsed }"
    type="button"
    :aria-label="ariaLabel"
    @click="emit('toggle')"
  >
    <SolarIcon class="tt-icon" :size="18" :name="modeIcon" />
    <span v-if="!collapsed" class="tt-label">{{ modeLabel }}</span>
  </button>
</template>

<style scoped>
.theme-toggle {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 0;
  width: 40px;
  height: 40px;
  padding: 0;
  border: none;
  border-radius: 10px;
  background: transparent;
  color: var(--text-regular);
  cursor: pointer;
  white-space: nowrap;
  transition: var(--transition-color);
}

.theme-toggle:hover {
  background: var(--surface-2);
  color: var(--text);
}

.theme-toggle:active {
  background: var(--accent-weak);
  color: var(--accent);
}

/* 折叠态：仅图标并居中 */
.theme-toggle.is-collapsed {
  justify-content: center;
  gap: 0;
  padding: 0;
}

.tt-icon {
  flex-shrink: 0;
}

.tt-label {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
}
</style>
