<!-- 继续观看：首页轮播右侧的紧凑卡片（2 行 × 3 列，封面 + 底部渐变文字 + 进度条） -->
<script setup lang="ts">
import { ref } from 'vue'
import { RouterLink } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import type { HistoryItem } from '@/types'

const props = defineProps<{
  /** 最近观看记录（store 已按 updatedAt 倒序并截取 6 条） */
  items: HistoryItem[]
}>()

const emit = defineEmits<{
  (e: 'play', item: HistoryItem): void
}>()

/** 封面加载失败回退首字用的集合（失败后不再重试该图） */
const failedCovers = ref<Record<string, boolean>>({})

function coverKey(item: HistoryItem): string {
  return `${item.siteKey}-${item.id}`
}

/** 无封面 / 加载失败时的首字占位 */
function initial(item: HistoryItem): string {
  return item.name.trim().charAt(0) || '影'
}

/** 观看进度百分比（无时长按 0 处理） */
function percent(item: HistoryItem): number {
  const { positionSec, durationSec } = item
  if (!durationSec || durationSec <= 0) return 0
  return Math.max(0, Math.min(100, Math.round((positionSec / durationSec) * 100)))
}
</script>

<template>
  <section
    v-if="props.items.length"
    class="continue-block"
    aria-label="继续观看"
  >
    <div class="continue-head">
      <h2 class="continue-title">
        继续观看
      </h2>
      <RouterLink
        class="continue-all"
        to="/history"
      >
        全部
        <SolarIcon
          name="alt-arrow-right"
          :size="12"
          aria-hidden="true"
        />
      </RouterLink>
    </div>
    <div class="continue-grid">
      <button
        v-for="item in props.items"
        :key="coverKey(item)"
        type="button"
        class="continue-card"
        :aria-label="`继续观看 ${item.name}${item.epTitle ? ` ${item.epTitle}` : ''}`"
        @click="emit('play', item)"
      >
        <span class="continue-cover">
          <img
            v-if="item.cover && !failedCovers[coverKey(item)]"
            class="continue-cover-img"
            :src="item.cover"
            :alt="item.name"
            loading="lazy"
            @error="failedCovers[coverKey(item)] = true"
          />
          <span
            v-else
            class="continue-cover-fallback"
          >{{ initial(item) }}</span>
          <span
            class="continue-shade"
            aria-hidden="true"
          />
          <span class="continue-text">
            <span class="continue-name ellipsis-1">{{ item.name }}</span>
            <span
              v-if="item.epTitle"
              class="continue-ep ellipsis-1"
            >{{ item.epTitle }}</span>
          </span>
          <span
            class="continue-bar"
            aria-hidden="true"
          >
            <span
              class="continue-bar-fill"
              :style="{ width: `${percent(item)}%` }"
            />
          </span>
        </span>
      </button>
    </div>
  </section>
</template>

<style scoped>
.continue-block {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  width: max-content;
  max-width: 100%;
  margin-left: auto;
  min-width: 0;
}

.continue-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-2);
}

.continue-title {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 600;
}

.continue-all {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  gap: 4px;
  color: var(--text-muted);
  font-size: 12px;
  transition: var(--transition-color);
}

.continue-all:hover {
  color: var(--accent);
}

/* 2 行 × 3 列；卡片固定 150px，不随窗口放大 */
.continue-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 150px));
  gap: var(--space-2);
  min-width: 0;
}

.continue-card {
  display: block;
  width: 100%;
  min-width: 0;
  padding: 0;
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-2);
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  overflow: hidden;
}

.continue-cover {
  position: relative;
  display: block;
  width: 100%;
  aspect-ratio: 3 / 4;
  overflow: hidden;
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.continue-cover-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 0.2s ease;
}

.continue-card:hover .continue-cover-img {
  transform: scale(1.05);
}

.continue-cover-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: var(--text-muted);
  font-size: 24px;
  font-weight: 600;
}

/* 底部渐变：保证白色片名在任何封面上的可读性 */
.continue-shade {
  position: absolute;
  inset: 0;
  background: linear-gradient(
    180deg,
    transparent 35%,
    color-mix(in srgb, #000 68%, transparent) 100%
  );
  pointer-events: none;
}

.continue-text {
  position: absolute;
  right: 8px;
  bottom: 8px;
  left: 8px;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
  color: #fff;
  pointer-events: none;
}

.continue-name {
  font-size: 12.5px;
  font-weight: 600;
  text-shadow: 0 1px 2px rgb(0 0 0 / 55%);
}

.continue-ep {
  font-size: 11px;
  opacity: 0.82;
  text-shadow: 0 1px 2px rgb(0 0 0 / 55%);
}

.continue-bar {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 3px;
  background: rgb(255 255 255 / 28%);
  pointer-events: none;
}

.continue-bar-fill {
  display: block;
  height: 100%;
  background: var(--accent);
  transition: width 0.2s ease;
}

@media (max-width: 1180px) {
  /* 轮播与继续观看上下堆叠时居中显示 */
  .continue-block {
    margin-inline: auto;
    margin-left: 0;
  }
}
</style>


