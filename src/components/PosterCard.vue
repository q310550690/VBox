<!-- 海报卡片（统一样式基座）：历史记录 / 收藏 / 搜索 / 点播 / 豆瓣 共用一张卡
     结构固定为「2:3 封面 + 标题 + 次要信息 + 弱化信息」，避免各页面各写一套样式：
     - 封面：懒加载 + 失败回退首字
     - 右上角角标：进度百分比（progress）/ 豆瓣评分（rating）
     - 封面底部进度条（progress > 0 时显示）
     - 封面 hover 操作区：由业务方通过 #actions 插槽传入（播放 / 删除 / 取消收藏…），
       按钮统一用 .action-btn 样式（见下方 :deep）
     只负责展示与点击语义；数据拼装、百分比/评分计算留在各业务卡片里。 -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'

/** 封面右上角角标 */
interface PosterCardBadge {
  text: string
  /** progress = 播放进度胶囊（白字）；rating = 评分（金色 + 星标） */
  tone?: 'progress' | 'rating'
}

const props = withDefaults(
  defineProps<{
    /** 片名（卡片标题，最多 2 行） */
    title: string
    /** 封面地址 */
    cover?: string | null
    /** 封面 alt；默认用 title */
    alt?: string
    /** 右上角角标 */
    badge?: PosterCardBadge | null
    /** 封面底部进度条百分比 0-100（不传 / 0 不显示） */
    progress?: number
    /** 次要信息一行（如 站点·集数 / 年份·地区）；传空串会保留一行空白以对齐网格 */
    subtitle?: string
    /** 弱化信息一行（如 日期·已看时长 / 简介） */
    description?: string
    /** 原生 title 提示（悬停提示，如「点击继续观看」） */
    hint?: string
    /** 禁用态：点击与键盘都不触发 open */
    disabled?: boolean
    /** 无障碍名称；默认由 title / subtitle / description 拼出 */
    ariaLabel?: string
  }>(),
  {
    cover: null,
    alt: undefined,
    badge: null,
    progress: 0,
    subtitle: undefined,
    description: undefined,
    hint: undefined,
    disabled: false,
    ariaLabel: undefined,
  },
)

const emit = defineEmits<{
  (e: 'open'): void
}>()

/** 封面加载失败标记：失败后回退为首字占位 */
const coverFailed = ref(false)

/** 切换条目时复位失败标记，避免图片地址变化后仍显示占位 */
watch(
  () => props.cover,
  () => {
    coverFailed.value = false
  },
)

const showCover = computed(() => Boolean(props.cover) && !coverFailed.value)
const initial = computed(() => props.title.trim().charAt(0) || '影')
const progressPercent = computed(() => {
  const value = Number.isFinite(props.progress) ? props.progress : 0
  return Math.max(0, Math.min(100, Math.round(value)))
})

const computedAriaLabel = computed(() => {
  if (props.ariaLabel) return props.ariaLabel
  const bits = [props.title, props.subtitle, props.description]
    .filter((value): value is string => Boolean(value && value.trim()))
  return bits.join('，')
})

function open() {
  if (props.disabled) return
  emit('open')
}

function onKeydown(event: KeyboardEvent) {
  // 只处理卡片本身获得焦点时的按键；hover 操作按钮内部的按键不冒泡触发卡片
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    open()
  }
}
</script>

<template>
  <div
    class="poster-card"
    :class="{ 'is-disabled': disabled }"
    role="button"
    :tabindex="disabled ? -1 : 0"
    :aria-disabled="disabled || undefined"
    :aria-label="computedAriaLabel"
    :title="hint"
    @click="open"
    @keydown="onKeydown"
  >
    <span class="cover">
      <img
        v-if="showCover"
        class="cover-img"
        :src="cover ?? ''"
        :alt="alt ?? title"
        loading="lazy"
        @error="coverFailed = true"
      />
      <span v-else class="cover-fallback" aria-hidden="true">{{ initial }}</span>

      <!-- 右上角角标 -->
      <span v-if="badge" class="badge" :class="`badge--${badge.tone ?? 'progress'}`">
        <SolarIcon v-if="badge.tone === 'rating'" name="star" :size="11" />
        {{ badge.text }}
      </span>

      <!-- 封面 hover 操作：业务按钮（播放 / 删除 / 取消收藏…） -->
      <span v-if="$slots.actions" class="cover-actions">
        <slot name="actions" />
      </span>

      <!-- 封面底部进度条 -->
      <span v-if="progressPercent > 0" class="cover-progress" aria-hidden="true">
        <span class="cover-progress-fill" :style="{ width: `${progressPercent}%` }" />
      </span>
    </span>

    <span class="meta">
      <span class="name">{{ title }}</span>
      <span v-if="subtitle !== undefined" class="line">{{ subtitle }}</span>
      <span v-if="description !== undefined" class="line line--desc">{{ description }}</span>
    </span>
  </div>
</template>

<style scoped>
/* 卡片外壳：白底 + 圆角 + 描边（与详情页 .panel 同一层级语言）；hover 只做描边 + 背景光晕，不做位移/缩放 */
.poster-card {
  display: flex;
  flex-direction: column;
  width: 100%;
  min-width: 0;
  margin: 0;
  padding: var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  background: var(--surface);
  color: inherit;
  font: inherit;
  text-align: left;
  overflow: hidden;
  cursor: pointer;
  transition:
    border-color 0.18s ease,
    background-color 0.18s ease,
    box-shadow 0.18s ease;
}

.poster-card:hover,
.poster-card:focus-visible {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 6%, var(--surface));
  box-shadow:
    0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent),
    0 6px 12px color-mix(in srgb, var(--accent) 22%, transparent);
  outline: none;
}

.poster-card.is-disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

/* 封面：2:3 + 圆角裁切（描边与阴影由卡片外壳负责） */
.cover {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  aspect-ratio: 2 / 3;
  overflow: hidden;
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.cover-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cover-fallback {
  color: var(--text-faint);
  font-size: 30px;
  font-weight: 600;
  line-height: 1;
  user-select: none;
}

/* 右上角角标：底色由 --bg 派生，亮暗主题下都能压住封面 */
.badge {
  position: absolute;
  top: 6px;
  right: 6px;
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, #000 58%, transparent);
  color: #fff;
  font-size: 11.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  line-height: 18px;
  text-shadow: 0 1px 2px rgb(0 0 0 / 45%);
  backdrop-filter: blur(4px);
}

/* 评分角标：金色 + 星标（豆瓣卡片） */
.badge--rating {
  color: #ffd66b;
}

/* 封面底部进度条 */
.cover-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  background: color-mix(in srgb, #000 35%, transparent);
}

.cover-progress-fill {
  display: block;
  height: 100%;
  background: var(--accent);
  transition: width 0.2s ease;
}

/* 封面 hover 操作区（插槽内容由业务卡片提供，样式统一在这里维护） */
.cover-actions {
  position: absolute;
  left: 50%;
  top: 50%;
  display: inline-flex;
  gap: 6px;
  transform: translate(-50%, -50%) scale(0.94);
  opacity: 0;
  pointer-events: none;
  transition:
    opacity 0.16s ease,
    transform 0.16s ease;
}

.poster-card:hover .cover-actions,
.poster-card:focus-visible .cover-actions {
  opacity: 1;
  transform: translate(-50%, -50%) scale(1);
  pointer-events: auto;
}

/* 操作按钮：深色半透明圆形；插槽内容在父组件作用域，用 :deep 统一覆盖 */
.cover-actions :deep(.action-btn) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: color-mix(in srgb, #000 62%, transparent);
  color: #fff;
  cursor: pointer;
  backdrop-filter: blur(4px);
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.cover-actions :deep(.action-btn:hover) {
  background: var(--accent);
  color: #fff;
}

/* 文字区：标题 2 行 + 次要信息 + 弱化信息 */
.meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 100%;
  min-width: 0;
  padding: var(--space-2) 2px 2px;
}

.name {
  display: -webkit-box;
  min-width: 0;
  max-width: 100%;
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 13px;
  font-weight: 500;
  line-height: 1.35;
  overflow-wrap: anywhere;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.line {
  display: block;
  min-width: 0;
  max-width: 100%;
  margin: 0;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11.5px;
  line-height: 1.4;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.line--desc {
  color: var(--text-faint);
}

@media (prefers-reduced-motion: reduce) {
  .poster-card,
  .cover-actions {
    transition: none;
  }

  .cover-progress-fill {
    transition: none;
  }
}
</style>
