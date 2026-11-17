<!-- 豆瓣首页内容区：轮播（热门电影 + 热门剧集）+ 右侧继续观看 → 豆瓣分类标签 → 卡片网格 -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import HomeCarousel from './HomeCarousel.vue'
import DoubanCard from './DoubanCard.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import ErrorState from '@/components/ErrorState.vue'
import type { DoubanCollection, DoubanHome, DoubanItem } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 豆瓣首页数据（null = 尚未加载） */
    home: DoubanHome | null
    loading?: boolean
    error?: string
    /** 是否在轮播右侧显示侧栏（继续观看）；无记录时轮播占满整行 */
    withSide?: boolean
  }>(),
  { loading: false, error: '', withSide: false },
)

const emit = defineEmits<{
  (e: 'select', item: DoubanItem): void
  (e: 'retry'): void
}>()

/** 当前选中的豆瓣分类 key */
const activeKey = ref('')

/** 分类列表（后端顺序即展示顺序） */
const collections = computed<DoubanCollection[]>(() => props.home?.collections ?? [])

/** 当前分类：activeKey 失效时回退到第一个分类 */
const activeCollection = computed<DoubanCollection | undefined>(() => {
  const list = collections.value
  if (!list.length) return undefined
  return list.find((c) => c.key === activeKey.value) ?? list[0]
})

/** 数据更新时校正选中项 */
watch(
  collections,
  (list) => {
    if (!list.length) {
      activeKey.value = ''
      return
    }
    if (!list.some((c) => c.key === activeKey.value)) activeKey.value = list[0].key
  },
  { immediate: true },
)

function onCategorySelect(key: string) {
  activeKey.value = key
}

function onCardSelect(item: DoubanItem) {
  emit('select', item)
}

/* ---------------- 状态展示 ---------------- */

const hasData = computed(() => Boolean(props.home))
const showError = computed(() => Boolean(props.error) && !hasData.value)
const showSkeleton = computed(() => props.loading && !hasData.value)

/** 「更新于 ...」：当天只显示 HH:mm，非当天带上日期（缓存可能是几天前的数据，只显示时间会误导） */
function updatedText(ms: number): string {
  if (!ms) return ''
  try {
    const d = new Date(ms)
    const now = new Date()
    const sameDay =
      d.getFullYear() === now.getFullYear() &&
      d.getMonth() === now.getMonth() &&
      d.getDate() === now.getDate()
    const time = d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
    if (sameDay) return time
    return `${d.getMonth() + 1}月${d.getDate()}日 ${time}`
  } catch {
    return ''
  }
}
</script>

<template>
  <div class="douban-home">
    <!-- 加载中（首次进入） -->
    <template v-if="showSkeleton">
      <div class="hero-skeleton" aria-label="首页加载中">
        <span class="hero-skeleton-cover" />
        <span class="hero-skeleton-lines">
          <span class="sk-line sk-line--title" />
          <span class="sk-line" />
          <span class="sk-line sk-line--short" />
          <span class="sk-line" />
        </span>
      </div>
      <SkeletonGrid :count="12" variant="card" />
    </template>

    <!-- 加载失败（且没有任何可展示数据） -->
    <ErrorState
      v-else-if="showError"
      :message="error"
      title="豆瓣首页加载失败"
      retry-text="重试"
      @retry="emit('retry')"
    />

    <template v-else-if="home">
      <!-- 轮播（60%）+ 右侧继续观看（40%，2 行 × 3 列） -->
      <div
        class="home-top"
        :class="{ 'has-side': withSide }"
      >
        <HomeCarousel
          :items="home.carousel"
          :image-proxy="home.imageProxy"
          @select="emit('select', $event)"
        />
        <aside
          v-if="withSide"
          class="home-side"
        >
          <slot name="side" />
        </aside>
      </div>

      <!-- 豆瓣分类标签 -->
      <div class="douban-tabs-block">
        <div
          class="douban-tabs"
          role="tablist"
          aria-label="豆瓣分类"
        >
          <button
            v-for="collection in collections"
            :key="collection.key"
            type="button"
            class="douban-tab"
            :class="{ 'is-active': collection.key === activeCollection?.key }"
            role="tab"
            :aria-selected="collection.key === activeCollection?.key"
            @click="onCategorySelect(collection.key)"
          >
            {{ collection.name }}
          </button>
        </div>
        <span
          v-if="home.fetchedAt"
          class="douban-updated"
        >
          数据来自豆瓣 · 更新于 {{ updatedText(home.fetchedAt) }}{{ loading ? ' · 正在更新…' : '' }}
        </span>
      </div>

      <!-- 当前分类卡片网格 -->
      <div
        v-if="activeCollection?.items.length"
        class="douban-grid"
      >
        <DoubanCard
          v-for="item in activeCollection.items"
          :key="item.id"
          :item="item"
          :image-proxy="home.imageProxy"
          @select="onCardSelect"
        />
      </div>
      <p
        v-else
        class="douban-empty"
      >
        该分类暂无内容，稍后再试或点击右上角刷新。
      </p>
    </template>
  </div>
</template>

<style scoped>
.douban-home {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  min-width: 0;
}

/* —— 顶部：轮播 60% + 继续观看 40% —— */
.home-top {
  /* 轮播的磨砂背景 hero-bg 以这里为定位容器，铺满整行（含右侧栏与列间隙） */
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--space-4);
  align-items: stretch;
  min-width: 0;
}

.home-top.has-side {
  grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
}

.home-side {
  /* 磨砂背景是 z-index: 0 的绝对定位层，这里抬一层保证文字始终在其上 */
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  justify-content: center;
  min-width: 0;
}

/* —— 卡片网格 —— */
.douban-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(158px, 1fr));
  gap: var(--space-3);
  min-width: 0;
}

/* 窗口较窄时：继续观看换到轮播下方，横向 6 列 */
@media (max-width: 1180px) {
  .home-top.has-side {
    grid-template-columns: minmax(0, 1fr);
  }
}

/* —— 加载骨架 —— */
.hero-skeleton {
  display: grid;
  grid-template-columns: 236px minmax(0, 1fr);
  gap: var(--space-6);
  align-items: center;
  min-height: 356px;
  padding: var(--space-5) var(--space-6);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-lg);
  background: var(--surface);
}

.hero-skeleton-cover {
  display: block;
  width: 208px;
  aspect-ratio: 2 / 3;
  border-radius: var(--radius-lg);
  background: var(--surface-2);
  animation: douban-pulse 1.2s ease-in-out infinite;
}

.hero-skeleton-lines {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.sk-line {
  display: block;
  width: 100%;
  max-width: 520px;
  height: 14px;
  border-radius: var(--radius-sm);
  background: var(--surface-2);
  animation: douban-pulse 1.2s ease-in-out infinite;
}

.sk-line--title {
  width: 42%;
  height: 30px;
}

.sk-line--short {
  width: 64%;
}

/* —— 豆瓣分类标签 —— */
.douban-tabs-block {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  min-width: 0;
}

.douban-tabs {
  display: flex;
  flex: 1;
  gap: var(--space-1);
  min-width: 0;
  overflow-x: auto;
  scrollbar-width: none;
}

.douban-tabs::-webkit-scrollbar {
  display: none;
}

.douban-tab {
  flex-shrink: 0;
  padding: 7px 12px;
  border: 0;
  border-bottom: 2px solid transparent;
  border-radius: var(--radius-sm) var(--radius-sm) 0 0;
  background: none;
  color: var(--text-muted);
  font: inherit;
  font-size: 13px;
  white-space: nowrap;
  cursor: pointer;
  transition:
    color 0.16s ease,
    background-color 0.16s ease,
    border-color 0.16s ease;
}

.douban-tab:hover {
  background: var(--surface-2);
  color: var(--text);
}

.douban-tab.is-active {
  border-bottom-color: var(--accent);
  color: var(--accent);
  font-weight: 600;
}

.douban-updated {
  flex-shrink: 0;
  color: var(--text-faint);
  font-size: 11.5px;
  white-space: nowrap;
}

.douban-empty {
  margin: var(--space-6) 0;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}

@keyframes douban-pulse {
  0%,
  100% {
    opacity: 0.55;
  }

  50% {
    opacity: 1;
  }
}

@media (max-width: 900px) {
  .hero-skeleton {
    grid-template-columns: 176px minmax(0, 1fr);
    gap: var(--space-4);
    padding: var(--space-4) var(--space-5);
  }

  .hero-skeleton-cover {
    width: 160px;
  }

  .douban-updated {
    display: none;
  }
}

@media (max-width: 680px) {
  .hero-skeleton {
    grid-template-columns: minmax(0, 1fr);
  }

  .hero-skeleton-cover {
    width: 132px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .hero-skeleton-cover,
  .sk-line {
    animation: none;
  }
}
</style>


