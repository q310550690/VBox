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
    class="rail-item theme-toggle"
    :class="{ 'is-collapsed': collapsed }"
    type="button"
    :aria-label="ariaLabel"
    :title="ariaLabel"
    @click="emit('toggle')"
  >
    <SolarIcon class="tt-icon" :size="18" :name="modeIcon" />
    <span v-if="!collapsed" class="tt-label">{{ modeLabel }}</span>
  </button>
</template>

<style scoped>
/* 尺寸 / 配色 / 悬停与选中全部复用 SideNav 的 .rail-item，
   保证展开态（34×34，只换色）与折叠态（40×40，铺 --surface-2）两套规矩一致。
   这里只留本组件自己的东西：图标不收缩、标签省略。 */
.theme-toggle {
  white-space: nowrap;
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
