<!-- 收藏页：筛选 + 卡片列表 + 四态（加载 / 错误 / 空 / 正常）
     桌面端数据来自 Rust SQLite 后端；浏览器预览为 mock + localStorage。 -->
<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import * as api from '@/api'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import HintBar from '@/components/HintBar.vue'
import StatusTabs from '@/components/StatusTabs.vue'
import FavoriteCard from './components/FavoriteCard.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import { useFavoritesStore } from '@/store/favorites'
import { useUiStore } from '@/store/ui'
import type { FavoriteFilter, FavoriteItem } from '@/types'

defineOptions({ name: 'FavoritesView' })

const router = useRouter()
const ui = useUiStore()
const store = useFavoritesStore()

/** 浏览器预览模式：收藏来自 mock + localStorage，需要明确标注 */
const isDemo = api.demoMode

/** 筛选 tab（与历史记录页共用 StatusTabs 组件） */
const FILTER_TABS: { value: FavoriteFilter; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'week', label: '本周' },
  { value: 'earlier', label: '更早' },
]

function onFilterChange(val: string) {
  if (val === 'all' || val === 'week' || val === 'earlier') {
    store.setFilter(val as FavoriteFilter)
  }
}

/** 打开详情（路由参数需 encodeURIComponent） */
function onOpen(item: FavoriteItem) {
  void router.push({
    name: 'favorites-detail',
    params: { site: item.siteKey, id: item.vodId },
  })
}

/** 取消收藏：即时移除并提示 */
async function onRemove(item: FavoriteItem) {
  try {
    await store.remove(item.siteKey, item.vodId)
    ui.notify('已取消收藏', 'success')
  } catch (e) {
    ui.notify('取消收藏失败：' + api.formatError(e), 'error')
  }
}

function onReload() {
  void store.load()
}

function onResetFilter() {
  store.setFilter('all')
}

function goVod() {
  void router.push('/vod')
}

onMounted(() => {
  void store.load()
})
</script>

<template>
  <PageScroll>
    <div class="page favorites-page">
    <div class="list-toolbar">
      <StatusTabs
        :model-value="store.filter"
        :items="FILTER_TABS"
        aria-label="收藏筛选"
        @update:model-value="onFilterChange"
      />
      <span class="toolbar-spacer" />
      <span v-if="isDemo" class="demo-tag">本地演示数据</span>
      <span class="muted total">共 {{ store.total }} 部</span>
      <el-tooltip content="刷新收藏" placement="bottom">
        <el-button
          circle
          :loading="store.loading"
          aria-label="刷新收藏"
          @click="onReload"
        >
          <template #icon><SolarIcon name="refresh" :size="14" /></template>
        </el-button>
      </el-tooltip>
    </div>

    <HintBar v-if="isDemo">
      浏览器预览模式：收藏保存在本地 localStorage，仅为演示数据；桌面端会写入本机 SQLite。
    </HintBar>

    <HintBar v-if="store.error && store.items.length" variant="error">
      刷新失败：{{ store.error }}
      <el-button text type="primary" @click="onReload">重试</el-button>
    </HintBar>

    <SkeletonGrid v-if="store.loading && !store.initialized" variant="card" :count="12" />

    <ErrorState
      v-else-if="store.error && !store.items.length"
      :message="store.error"
      @retry="onReload"
    />

    <EmptyState
      v-else-if="!store.items.length"
      title="还没有收藏"
      desc="在详情页点「收藏」，喜欢的影片会出现在这里。"
      action-text="去点播"
      @action="goVod"
    >
      <template #icon><SolarIcon name="star" :size="34" /></template>
    </EmptyState>

    <EmptyState
      v-else-if="!store.filteredItems.length"
      title="当前筛选下没有收藏"
      desc="换成「全部」或「更早」再看看。"
      action-text="查看全部"
      @action="onResetFilter"
    />

    <div v-else class="favorites-grid">
      <FavoriteCard
        v-for="item in store.filteredItems"
        :key="`${item.siteKey}-${item.vodId}`"
        :item="item"
        @open="onOpen"
        @remove="onRemove"
      />
    </div>
    </div>
  </PageScroll>
</template>

<style scoped>
.list-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}

.toolbar-spacer {
  flex: 1;
  min-width: var(--space-2);
}

.total {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.demo-tag {
  padding: 1px var(--space-2);
  border: 1px solid var(--warning);
  border-radius: var(--radius-pill);
  background: var(--warning-weak);
  color: var(--warning);
  font-size: 11px;
  line-height: 18px;
}

/* 与历史记录页一致：自适应海报网格 */
.favorites-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(158px, 1fr));
  gap: var(--space-3);
  min-width: 0;
}
</style>
