<!-- 统一图标组件：所有界面图标都从这里渲染，图标源为 src/assets/solar-linear/（Solar Linear 线性图标）
     - 用法：<SolarIcon name="play" :size="18" />，name 见 src/icons/solar.ts 的 SolarIconName
     - 颜色跟随 CSS `color`/`currentColor`，大小由 size（px 或 CSS 长度）决定
     - 需要转圈（loading）时传 spin -->
<script setup lang="ts">
import { computed } from 'vue'
import { solarIcons, type SolarIconName } from '@/icons/solar'

const props = withDefaults(
  defineProps<{
    /** 图标名（Solar Linear，见 src/icons/solar.ts） */
    name: SolarIconName
    /** 图标尺寸，数字按 px 处理；不传则跟随父级字号（等价 Element Plus el-icon 的 1em） */
    size?: number | string
    /** 是否旋转（loading 类图标） */
    spin?: boolean
    /** 无障碍名称；不传则视为纯装饰图标（aria-hidden） */
    label?: string
  }>(),
  {
    size: undefined,
    spin: false,
    label: undefined,
  },
)

const svg = computed(() => solarIcons[props.name])
const fontSize = computed(() =>
  props.size === undefined
    ? undefined
    : typeof props.size === 'number'
      ? `${props.size}px`
      : props.size,
)
</script>

<template>
  <span
    class="solar-icon"
    :class="{ 'is-spin': spin }"
    :style="{ fontSize }"
    :role="label ? 'img' : undefined"
    :aria-label="label"
    :aria-hidden="label ? undefined : 'true'"
    v-html="svg"
  />
</template>

<style>
/* 图标内部 svg 原样是 width/height = 1em，这里统一撑满外层 span 的 1em 方块 */
.solar-icon > svg {
  display: block;
  width: 1em;
  height: 1em;
}

.solar-icon.is-spin > svg {
  animation: solar-icon-spin 1s linear infinite;
}

@keyframes solar-icon-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>

<style scoped>
.solar-icon {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 1em;
  height: 1em;
  line-height: 1;
  vertical-align: -0.125em;
}
</style>
