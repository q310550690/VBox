<!-- 历史记录页（frontend-design.md §6.6）：筛选 + 清空 + 进度列表
     桌面端数据来自 Rust SQLite 后端；浏览器预览为 mock + localStorage（页头标注「本地演示数据」）。
     后端语义：每个「站点 + 影片」只保留一行，新集覆盖旧行。 -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import * as api from '@/api'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import HintBar from '@/components/HintBar.vue'
import StatusTabs from '@/components/StatusTabs.vue'
import PageHead from '@/components/PageHead.vue'
import HistoryCard from './components/HistoryCard.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import { useConfigStore } from '@/store/config'
import { useHistoryStore } from '@/store/history'
import { useUiStore } from '@/store/ui'
import type { HistoryFilter, HistoryItem } from '@/types'

defineOptions({ name: 'HistoryView' })

const router = useRouter()
const ui = useUiStore()
const config = useConfigStore()
const store = useHistoryStore()

/** 浏览器预览模式：历史来自 mock + localStorage，需要明确标注 */
const isDemo = api.demoMode

/** 清空历史的二次确认 */
const clearOpen = ref(false)

/** 筛选 tab（与下载页共用 StatusTabs 组件） */
const FILTER_TABS: { value: HistoryFilter; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'week', label: '本周' },
  { value: 'earlier', label: '更早' },
]

function onFilterChange(val: string) {
  if (val === 'all' || val === 'week' || val === 'earlier') store.setFilter(val as HistoryFilter)
}

/** 继续观看：进详情页并带上「第几集 + 已看秒数」，由详情页自动选中并起播。
 *
 * 旧版本可能写入过缺站点 key / 站点已从订阅源移除的记录：这类记录跳过去只会落到
 * 兜底路由跳回首页（用户感知就是「点了没反应 / 跳首页」），这里提前提示并忽略；
 * 记录本身仍可通过右上角删除按钮清掉。 */
function onPlay(item: HistoryItem) {
  const siteKey = (item.siteKey || '').trim()
  const id = (item.id || '').trim()
  if (!siteKey || !id) {
    ui.notify('该记录来源已失效（缺少站点或影片标识），无法继续播放，可删除这条记录', 'warning')
    return
  }
  // 配置已成功加载时顺带校验站点仍然存在；配置未加载/加载失败则不阻断
  //（详情页会给出明确错误，避免因订阅源临时不可用把整页历史都变成「点不动」）。
  if (
    config.initialized &&
    !config.error &&
    config.sites.length > 0 &&
    !config.siteByKey(siteKey)
  ) {
    ui.notify('该记录所属站点已不存在（可能订阅源已更换），可删除这条记录', 'warning')
    return
  }
  void router.push({
    name: 'history-detail',
    params: { site: siteKey, id },
    query: {
      ep: String(item.epIndex),
      t: String(Math.max(0, Math.round(item.positionSec || 0))),
    },
  })
}

/** 单条删除：即时移除并提示（按站点 + 影片删除） */
async function onRemove(item: HistoryItem) {
  try {
    await store.remove(item.siteKey, item.id)
    ui.notify('已删除该条观看记录', 'success')
  } catch (e) {
    ui.notify('删除失败：' + api.formatError(e), 'error')
  }
}

/** 清空全部历史（ConfirmDialog 确认后执行） */
async function onClearConfirm() {
  try {
    await store.clear()
    clearOpen.value = false
    ui.notify('已清空观看历史', 'success')
  } catch (e) {
    ui.notify('清空失败：' + api.formatError(e), 'error')
  }
}

function onResetFilter() {
  store.setFilter('all')
}

function goVod() {
  void router.push('/vod')
}

onMounted(() => {
  void store.load()
  void store.loadSettings()
})
</script>

<template>
  <PageScroll>
    <div class="page history-page">
    <PageHead class="list-toolbar" wrap>
      <StatusTabs
        :model-value="store.filter"
        :items="FILTER_TABS"
        aria-label="历史记录筛选"
        @update:model-value="onFilterChange"
      />
      <span class="toolbar-spacer" />
      <template #tools>
      <div class="page-toolbar page-toolbar--meta">
        <span class="retention" :title="`当前保留策略：${store.retentionLabel}`">
          <SolarIcon :size="12" name="clock-circle" aria-hidden="true" />
          保留：{{ store.retentionLabel }}
        </span>
        <span v-if="isDemo" class="demo-tag">本地演示数据</span>
        <span class="muted total">共 {{ store.total }} 条</span>
        <span class="page-toolbar__sep" aria-hidden="true" />
        <el-tooltip content="清空历史记录" placement="bottom">
          <el-button
            circle
            type="danger"
            :disabled="store.isEmpty || store.loading"
            aria-label="清空历史记录"
            @click="clearOpen = true"
          >
            <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
          </el-button>
        </el-tooltip>
      </div>
      </template>
    </PageHead>

    <HintBar v-if="isDemo">
      浏览器预览模式：历史记录保存在本地 localStorage，仅为演示数据；桌面端会写入本机 SQLite。
    </HintBar>

    <SkeletonGrid v-if="store.loading && !store.initialized" variant="card" :count="12" />

    <ErrorState
      v-else-if="store.error && !store.items.length"
      :message="store.error"
      @retry="store.load"
    />

    <EmptyState
      v-else-if="!store.items.length"
      title="还没有观看记录"
      desc="去点播页逛一逛，看过的内容会自动记录在这里。"
      action-text="去点播"
      @action="goVod"
    />

    <EmptyState
      v-else-if="!store.filteredItems.length"
      title="当前筛选下没有记录"
      desc="换成「全部」或「更早」再看看。"
      action-text="查看全部"
      @action="onResetFilter"
    />

    <template v-else>
      <HintBar v-if="store.error" variant="error">
        刷新失败：{{ store.error }}
        <el-button text type="primary" @click="store.load">重试</el-button>
      </HintBar>
      <div class="history-grid">
        <HistoryCard
          v-for="item in store.filteredItems"
          :key="`${item.siteKey}-${item.id}`"
          :item="item"
          @play="onPlay"
          @remove="onRemove"
        />
      </div>
    </template>

    <!-- 清空历史二次确认 -->
    <ConfirmDialog
      :open="clearOpen"
      title="清空观看历史"
      message="确认清空全部历史？此操作不可撤销"
      confirm-text="清空"
      cancel-text="取消"
      danger
      @confirm="onClearConfirm"
      @cancel="clearOpen = false"
      @update:open="clearOpen = $event"
    />
    </div>
  </PageScroll>
</template>

<style scoped>
.retention {
  display: inline-flex;
  align-items: center;
  gap: var(--space-1);
  font-size: 12px;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.retention .solar-icon {
  color: var(--text-faint);
}

.demo-tag {
  padding: 1px var(--space-2);
  border: 0;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--el-color-warning) 12%, transparent);
  color: color-mix(in srgb, var(--el-color-warning) 80%, var(--el-text-color-primary));
  font-size: 11px;
  line-height: 18px;
}

.total {
  font-size: 12px;
}

/* 与首页（豆瓣）卡片一致：自适应海报网格 */
.history-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(158px, 1fr));
  gap: var(--space-3);
  min-width: 0;
}
</style>
