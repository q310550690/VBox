<!-- 首页轮播：左侧堆叠封面（扇形堆叠 + 位移动画），右侧影视信息 / 简介（切换时文字渐隐渐显）。
     背景使用当前封面「模糊 + 渐隐」融入 APP 背景，不再用卡片底色区分。 -->
<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import { proxyImage } from '@/utils/image'
import type { DoubanItem } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 轮播条目（10 条：热门电影 + 热门剧集） */
    items: DoubanItem[]
    /** 本机图片代理前缀（豆瓣封面需 Referer） */
    imageProxy?: string | null
  }>(),
  { imageProxy: null },
)

const emit = defineEmits<{
  (e: 'select', item: DoubanItem): void
}>()

/** 自动轮播间隔 */
const INTERVAL_MS = 6000

const index = ref(0)
const active = computed<DoubanItem | undefined>(() => props.items[index.value])
const hasMultiple = computed(() => props.items.length > 1)

/** 封面加载失败集合（失败后回退首字占位，不反复请求） */
const failed = ref<Record<string, boolean>>({})

function coverOf(item: DoubanItem): string {
  return proxyImage(item.cover, props.imageProxy)
}

function initialOf(item: DoubanItem): string {
  return item.title.trim().charAt(0) || '影'
}

function ratingText(item: DoubanItem): string {
  return typeof item.rating === 'number' && item.rating > 0 ? item.rating.toFixed(1) : ''
}

/** 类型文案：豆瓣 subtype → 中文（轮播只含电影 / 剧集） */
function kindOf(item: DoubanItem): string {
  if (item.subtype === 'tv') return '剧集'
  if (item.subtype === 'show') return '综艺'
  return '电影'
}

/** 把 info（地区 / 类型 / 导演 / 主演）拆成段 */
function infoParts(item: DoubanItem): string[] {
  return (item.info ?? '')
    .split(' / ')
    .map((part) => part.trim())
    .filter(Boolean)
}

/** 年份 + 地区 + 类型：单独一行 */
function metaLine(item: DoubanItem): string {
  const parts = infoParts(item)
  return [item.year, parts[0], parts[1]].filter(Boolean).join(' · ')
}

/** 导演 / 主演：单独一行弱化展示 */
function castLine(item: DoubanItem): string {
  return infoParts(item).slice(2).join(' · ')
}

/** 背景模糊层：当前封面（不参与交互） */
const backdrop = computed(() => {
  if (!active.value) return ''
  const cover = coverOf(active.value)
  return cover ? `url("${cover.replace(/"/g, '%22')}")` : ''
})

/**
 * 堆叠卡片（参考 Swiper effect-cards）：只在右侧堆叠 3 张，静止时左侧不显示卡片；
 * delta=-1 是「上一张」，静止时透明并停在左侧，切到下一张时从当前位置飞出并淡出。
 */
interface StackCard {
  item: DoubanItem
  index: number
  delta: number
}

/** 右侧最多堆叠几张（不含最前） */
const STACK_BEHIND = 2

const stack = computed<StackCard[]>(() => {
  const total = props.items.length
  if (!total) return []
  const list: StackCard[] = []
  // 上一张：只用于切换动画（静止时 opacity 0）
  if (total > 1) {
    const prev = (((index.value - 1) % total) + total) % total
    list.push({ item: props.items[prev], index: prev, delta: -1 })
  }
  // 当前 + 右侧后方若干张
  const behind = Math.min(STACK_BEHIND, total - 1)
  for (let delta = 0; delta <= behind; delta += 1) {
    const i = (index.value + delta) % total
    list.push({ item: props.items[i], index: i, delta })
  }
  return list
})

function stackStyle(card: StackCard): Record<string, string> {
  const delta = card.delta
  // 最前
  if (delta === 0) {
    return {
      transform: 'translate3d(0, 0, 0) rotate(0deg) scale(1)',
      zIndex: '10',
      opacity: '1',
    }
  }
  // 上一张：停靠在左侧、透明；切换时过渡成飞出/飞入
  if (delta < 0) {
    return {
      transform: 'translate3d(-78%, -14px, 0) rotate(-16deg) scale(1.04)',
      zIndex: '12',
      opacity: '0',
    }
  }
  // 右侧堆叠：越靠后越往右下、旋转越大、缩放越小
  const offset = delta
  return {
    transform: `translate3d(${offset * 14}px, ${offset * 9}px, 0) rotate(${offset * 3}deg) scale(${1 - offset * 0.05})`,
    zIndex: String(10 - offset),
    opacity: offset >= 2 ? '0.72' : '1',
  }
}

/* ---------------- 切换 / 自动轮播 ---------------- */

let timer: number | null = null
const paused = ref(false)
const activated = ref(true)

function clearTimer() {
  if (timer !== null) {
    window.clearTimeout(timer)
    timer = null
  }
}

function schedule() {
  clearTimer()
  if (!hasMultiple.value || paused.value || !activated.value) return
  timer = window.setTimeout(() => {
    index.value = (index.value + 1) % props.items.length
    schedule()
  }, INTERVAL_MS)
}

function go(step: number) {
  if (!hasMultiple.value) return
  const total = props.items.length
  index.value = (((index.value + step) % total) + total) % total
  schedule()
}

function goTo(target: number) {
  if (target === index.value) return
  index.value = target
  schedule()
}

function onCardClick(card: StackCard) {
  if (card.delta === 0) emit('select', card.item)
  else goTo(card.index)
}

function onSelect() {
  if (active.value) emit('select', active.value)
}

function pause() {
  paused.value = true
  clearTimer()
}

function resume() {
  paused.value = false
  schedule()
}

watch(
  () => props.items,
  () => {
    index.value = 0
    failed.value = {}
    schedule()
  },
)

watch(
  () => props.items.length,
  () => schedule(),
)

onMounted(schedule)
onBeforeUnmount(clearTimer)
onActivated(() => {
  activated.value = true
  schedule()
})
onDeactivated(() => {
  activated.value = false
  clearTimer()
})
</script>

<template>
  <section
    v-if="active"
    class="home-hero"
    aria-label="热门影视轮播"
    @mouseenter="pause"
    @mouseleave="resume"
    @focusin="pause"
    @focusout="resume"
  >
    <!-- 磨砂背景：当前封面模糊，铺满整个 home-top（含右侧栏与间隙），四周以透明度渐隐 -->
    <div
      class="hero-bg"
      aria-hidden="true"
    >
      <Transition name="hero-bg-fade">
        <div
          :key="active.id"
          class="hero-bg-img"
          :style="{ backgroundImage: backdrop }"
        />
      </Transition>
    </div>

    <!-- 左侧：堆叠封面 + 评分角标 -->
    <div class="hero-stage">
      <button
        v-for="card in stack"
        :key="card.item.id"
        type="button"
        class="stack-card"
        :class="{ 'is-active': card.delta === 0, 'is-prev': card.delta < 0 }"
        :style="stackStyle(card)"
        :aria-label="card.delta === 0 ? `查看《${card.item.title}》` : `切换到《${card.item.title}》`"
        :tabindex="card.delta === 0 ? 0 : -1"
        @click="onCardClick(card)"
      >
        <img
          v-if="coverOf(card.item) && !failed[card.item.id]"
          class="stack-cover"
          :src="coverOf(card.item)"
          :alt="card.item.title"
          loading="lazy"
          draggable="false"
          @error="failed[card.item.id] = true"
        />
        <span v-else class="stack-fallback">{{ initialOf(card.item) }}</span>
        <span
          v-if="card.delta === 0 && ratingText(card.item)"
          class="stack-rating"
        >
          <SolarIcon
            name="star"
            :size="11"
          />
          {{ ratingText(card.item) }}
        </span>
      </button>
    </div>

    <!-- 右侧：影视信息 + 简介（切换时渐隐渐显） -->
    <div class="hero-info">
      <Transition name="hero-fade" mode="out-in">
        <div
          :key="active.id"
          class="hero-info-inner"
        >
          <span class="hero-kicker">
            {{ kindOf(active) }} · 豆瓣热播
          </span>
          <h2 class="hero-title">
            {{ active.title }}
          </h2>
          <p
            v-if="metaLine(active)"
            class="hero-meta"
          >
            {{ metaLine(active) }}
          </p>
          <p
            v-if="castLine(active)"
            class="hero-cast"
          >
            {{ castLine(active) }}
          </p>
          <p class="hero-intro">
            {{ active.intro || '' }}
          </p>
          <div class="hero-actions">
            <el-button
              type="primary"
              round
              @click="onSelect"
            >
              <template #icon>
                <SolarIcon
                  name="magnifier"
                  size="1em"
                />
              </template>
              搜索片源
            </el-button>
          </div>
        </div>
      </Transition>

      <!-- 控制条：上一张 / 圆点 / 下一张 -->
      <div
        v-if="hasMultiple"
        class="hero-controls"
      >
        <button
          type="button"
          class="hero-arrow"
          aria-label="上一张"
          @click="go(-1)"
        >
          <SolarIcon
            name="alt-arrow-left"
            :size="16"
          />
        </button>
        <div
          class="hero-dots"
          role="tablist"
          aria-label="轮播切换"
        >
          <button
            v-for="(item, i) in items"
            :key="item.id"
            type="button"
            class="hero-dot"
            :class="{ 'is-active': i === index }"
            :aria-label="`第 ${i + 1} 张：${item.title}`"
            :aria-selected="i === index"
            role="tab"
            @click="goTo(i)"
          />
        </div>
        <button
          type="button"
          class="hero-arrow"
          aria-label="下一张"
          @click="go(1)"
        >
          <SolarIcon
            name="alt-arrow-right"
            :size="16"
          />
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 整体：左封面舞台 + 右信息区；背景是封面的模糊磨砂，铺满 .home-top 整行 */
.home-hero {
  /* 不设 position：磨砂层 hero-bg 要越过本格，以父级 .home-top 为定位容器 */
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: 60px;
  align-items: center;
  padding: var(--space-2) 0;
}

/* —— 磨砂背景（封面模糊铺满 home-top，含右侧栏） —— */
.hero-bg {
  position: absolute;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  pointer-events: none;
  /* 圆形渐隐（恢复之前的圆云形状，过渡带拉长：实心 30% → 透明 90% 半径）。
     只用透明度遮罩、不画任何底色，且透明点内缩到四边之内 ——
     .app-content 不论不透明还是透明，四周都只是 alpha 归零，不会露出硬边 */
  mask-image: radial-gradient(ellipse 46% 50% at 42% 47%, #000 30%, transparent 90%);
  -webkit-mask-image: radial-gradient(ellipse 46% 50% at 42% 47%, #000 30%, transparent 90%);
}

.hero-bg-img {
  position: absolute;
  inset: 0;
  background-position: center;
  background-size: cover;
  opacity: 0.36;
  filter: blur(46px) saturate(1.25);
  transform: scale(1.06);
}

.hero-bg-fade-enter-active,
.hero-bg-fade-leave-active {
  transition: opacity 0.8s ease;
}

.hero-bg-fade-enter-from,
.hero-bg-fade-leave-to {
  opacity: 0;
}

/* —— 左侧堆叠封面：高度固定 460px，全屏时不再随窗口放大 —— */
.hero-stage {
  position: relative;
  z-index: 1;
  width: 306.67px;
  height: 460px;
  justify-self: start;
  perspective: 1200px;
}

.stack-card {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  padding: 0;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--surface) 60%, var(--border));
  border-radius: var(--radius-lg);
  background: var(--surface-2);
  box-shadow: 0 18px 40px color-mix(in srgb, var(--bg) 65%, transparent);
  cursor: pointer;
  transform-origin: 50% 60%;
  transition:
    transform 0.62s cubic-bezier(0.22, 0.68, 0.32, 1),
    opacity 0.55s ease,
    box-shadow 0.3s ease;
  will-change: transform, opacity;
}

.stack-card.is-active:hover {
  box-shadow: 0 22px 46px color-mix(in srgb, var(--accent) 26%, transparent);
}

/* 静止时停在左侧的「上一张」不可点击（只用于切换动画） */
.stack-card.is-prev {
  pointer-events: none;
}

.stack-cover {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
  user-select: none;
}

.stack-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  color: var(--text-faint);
  font-size: 40px;
  font-weight: 600;
}

/* 封面右上角评分角标 */
.stack-rating {
  position: absolute;
  top: 8px;
  right: 8px;
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 2px 7px;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, #000 58%, transparent);
  color: #ffd66b;
  font-size: 12px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  line-height: 18px;
  text-shadow: 0 1px 2px rgb(0 0 0 / 45%);
  backdrop-filter: blur(4px);
  pointer-events: none;
}

/* —— 右侧信息 —— */
.hero-info {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: var(--space-3);
  min-width: 0;
  min-height: 268px;
}

.hero-info-inner {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
}

.hero-kicker {
  color: var(--accent);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.08em;
}

.hero-title {
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: clamp(24px, 2.2vw, 32px);
  font-weight: 700;
  line-height: 1.2;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 年份 / 地区 / 类型：单独一行 */
.hero-meta {
  margin: 0;
  overflow: hidden;
  color: var(--text-regular);
  font-size: 13px;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 导演 / 主演：单独一行，弱化 */
.hero-cast {
  margin: 0;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 12px;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 剧情简介：固定 3 行高度，超出省略号（不足 3 行也保持高度，避免切换时布局跳动） */
.hero-intro {
  display: -webkit-box;
  height: calc(1.75em * 3);
  max-width: 660px;
  margin: var(--space-1) 0 0;
  overflow: hidden;
  color: var(--text-regular);
  font-size: 13px;
  line-height: 1.75;
  overflow-wrap: anywhere;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 3;
}

.hero-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-1);
}

/* —— 控制条 —— */
.hero-controls {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.hero-arrow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: 50%;
  background: color-mix(in srgb, var(--surface) 70%, transparent);
  color: var(--text-muted);
  cursor: pointer;
  transition:
    border-color 0.16s ease,
    color 0.16s ease,
    background-color 0.16s ease;
}

.hero-arrow:hover {
  border-color: var(--accent);
  background: var(--accent-weak);
  color: var(--accent);
}

.hero-dots {
  display: flex;
  align-items: center;
  gap: 6px;
}

.hero-dot {
  width: 6px;
  height: 6px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: var(--border);
  cursor: pointer;
  transition:
    width 0.2s ease,
    background-color 0.2s ease;
}

.hero-dot.is-active {
  width: 18px;
  border-radius: var(--radius-pill);
  background: var(--accent);
}

/* 文字渐隐 / 渐显 */
.hero-fade-enter-active {
  transition:
    opacity 0.34s ease,
    transform 0.34s cubic-bezier(0.22, 0.68, 0.32, 1);
}

.hero-fade-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}

.hero-fade-enter-from {
  opacity: 0;
  transform: translateY(10px);
}

.hero-fade-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}

/* 窄窗口：缩小封面与间距（简介仍保持固定 3 行） */
@media (max-width: 900px) {
  .home-hero {
    grid-template-columns: auto minmax(0, 1fr);
    gap: 44px;
    padding: var(--space-3) var(--space-4);
  }

  .hero-stage {
    height: 420px;
    width: 280px;
  }
}

@media (max-width: 680px) {
  .home-hero {
    grid-template-columns: minmax(0, 1fr);
  }

  .hero-stage {
    height: 380px;
    width: 253.33px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .stack-card,
  .hero-bg-img,
  .hero-fade-enter-active,
  .hero-fade-leave-active,
  .hero-dot {
    transition: none;
  }
}
</style>






