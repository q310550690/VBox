<!-- 设置页 · 源健康：站点/线路连续失败记录（变灰禁用来源、可手动重置） -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { formatRelativeTime } from '@/utils/format'
import { CATVOD_ERROR_COPY, translateCatvodError } from '@/utils/catvodErrors'
import type { DailyStat, ErrorCodeStat, ErrorEvent, SourceHealth } from '@/types'
import { useConfigStore } from '@/store/config'
import { groupApiFailures } from '@/utils/sourceHealthStats'

const items = ref<SourceHealth[]>([])
const codeStats = ref<ErrorCodeStat[]>([])
const recentEvents = ref<ErrorEvent[]>([])
const dailyStats = ref<DailyStat[]>([])
const config = useConfigStore()
const loading = ref(false)
const resettingKey = ref('')
const resettingAll = ref(false)
const error = ref('')

const disabledCount = computed(() => items.value.filter((item) => item.disabled).length)

async function load() {
  if (api.demoMode) return
  loading.value = true
  error.value = ''
  try {
    const [nextItems, nextStats, nextEvents, nextDaily] = await Promise.all([
      api.sourceHealthList(),
      api.sourceHealthCodeStats(),
      api.sourceHealthRecentEvents(),
      api.sourceHealthDaily(7),
    ])
    items.value = nextItems
    codeStats.value = nextStats
    recentEvents.value = nextEvents
    dailyStats.value = nextDaily
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

const itemKey = (item: SourceHealth) => `${item.kind}:${item.key}`
const kindLabel = (item: SourceHealth) => (item.kind === 'line' ? '线路' : '站点')
const itemTitle = (item: SourceHealth) => item.label.trim() || item.key
const codeLabel = (code: string) => CATVOD_ERROR_COPY[code]?.title ?? code
const codeAction = (code: string) => CATVOD_ERROR_COPY[code]?.action ?? ''
const errorText = (message?: string) =>
  message ? (translateCatvodError(message) ?? message) : ''
const dailyMax = computed(() =>
  Math.max(1, ...dailyStats.value.map((item) => item.ok + item.fail)),
)
const dailyBarWidth = (item: DailyStat) => `${Math.round(((item.ok + item.fail) / dailyMax.value) * 100)}%`
const dailyOkWidth = (item: DailyStat) => {
  const total = item.ok + item.fail
  return total === 0 ? '0%' : `${Math.round((item.ok / total) * 100)}%`
}
const dayLabel = (day: string) => day.slice(5)
const apiFailures = computed(() => groupApiFailures(items.value, config.sites, 8))

async function resetOne(item: SourceHealth) {
  if (resettingKey.value) return
  resettingKey.value = itemKey(item)
  try {
    await api.sourceHealthReset(item.kind, item.key)
    await load()
    // 同步全局健康快照：站点选择器/首页的 isSiteDisabled 依赖 store.config.health
    await config.loadHealth()
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    resettingKey.value = ''
  }
}

async function resetAll() {
  if (resettingAll.value) return
  resettingAll.value = true
  try {
    await api.sourceHealthResetAll()
    await load()
    await config.loadHealth()
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    resettingAll.value = false
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-source-health">
    <header class="section-head">
      <div class="section-head-text">
        <h2 id="settings-source-health" class="section-title">源健康</h2>
        <p class="section-desc">站点/线路连续失败 3 次后自动变灰禁用；成功一次即自动恢复。</p>
      </div>
      <el-button
        :loading="loading || resettingAll"
        :disabled="api.demoMode || !items.length"
        @click="resetAll"
      >
        <template #icon><SolarIcon name="restart" /></template>
        全部重置
      </el-button>
    </header>

    <HintBar v-if="api.demoMode">浏览器预览模式无真实源请求，源健康记录为空。</HintBar>
    <HintBar v-if="error" variant="error">读取/重置源健康失败：{{ error }}</HintBar>

    <p v-if="!api.demoMode && !items.length && !loading" class="health-empty">
      暂无失败记录。源请求失败会自动记录，连续失败达到阈值后会在点播页变灰并自动切换。
    </p>

    <el-scrollbar v-else-if="items.length" class="health-scroll" max-height="360px">
      <ul class="health-list" aria-label="源健康记录">
        <li v-for="item in items" :key="itemKey(item)" class="health-row">
          <div class="health-main">
            <div class="health-title-line">
              <span class="health-kind">{{ kindLabel(item) }}</span>
              <span class="health-name ellipsis-1" :title="itemTitle(item)">{{ itemTitle(item) }}</span>
              <span
                class="health-badge"
                :class="item.disabled ? 'is-disabled' : 'is-ok'"
              >
                {{ item.disabled ? '已禁用' : '可用' }}
              </span>
            </div>
            <p class="health-meta">
              连续失败 <strong>{{ item.failures }}</strong> 次 · 累计 {{ item.totalFailures }} 次 ·
              {{ formatRelativeTime(item.updatedAt) }}
            </p>
            <p v-if="item.lastError" class="health-error ellipsis-1" :title="item.lastError">
              最近错误：{{ errorText(item.lastError) }}
            </p>
          </div>
          <el-button
            :loading="resettingKey === itemKey(item)"
            :disabled="api.demoMode || item.failures === 0"
            @click="resetOne(item)"
          >
            <template #icon><SolarIcon name="restart" /></template>
            重置
          </el-button>
        </li>
      </ul>
    </el-scrollbar>

    <p v-if="disabledCount > 0" class="health-note">
      当前有 {{ disabledCount }} 个源因连续失败被禁用；点播页会自动跳过它们。
    </p>

    <div v-if="dailyStats.length" class="health-daily">
      <p class="health-codes-title">近 7 天成败</p>
      <ul class="health-daily-list">
        <li v-for="item in dailyStats" :key="item.day" class="health-daily-row">
          <span class="health-daily-day">{{ dayLabel(item.day) }}</span>
          <span class="health-daily-track" :style="{ width: dailyBarWidth(item) }">
            <span v-if="item.ok" class="health-daily-ok" :style="{ width: dailyOkWidth(item) }" />
            <span v-if="item.fail" class="health-daily-fail" :style="{ width: `${100 - Number.parseFloat(dailyOkWidth(item))}%` }" />
          </span>
          <span class="health-daily-meta">成功 {{ item.ok }} · 失败 {{ item.fail }}</span>
        </li>
      </ul>
    </div>

    <div v-if="recentEvents.length" class="health-events">
      <p class="health-codes-title">最近失败事件</p>
      <ul class="health-event-list">
        <li v-for="(event, index) in recentEvents.slice(0, 6)" :key="`${event.createdAt}-${index}`" class="health-event-row">
          <span class="health-event-time">{{ formatRelativeTime(event.createdAt) }}</span>
          <span class="health-event-name ellipsis-1" :title="event.key">{{ event.label || event.key }}</span>
          <span class="health-event-code" :title="event.message ? errorText(event.message) : (event.code ?? '')">{{ event.code ? codeLabel(event.code) : '未识别' }}</span>
        </li>
      </ul>
    </div>

    <div v-if="codeStats.length" class="health-codes">
      <p class="health-codes-title">错误码分布（最近一次失败）</p>
      <ul class="health-code-list">
        <li v-for="stat in codeStats" :key="stat.code" class="health-code-row">
          <span class="health-code-name" :title="codeAction(stat.code)">{{ codeLabel(stat.code) }}</span>
          <span class="health-code-meta">{{ stat.sources }} 个源 · 累计 {{ stat.failures }} 次</span>
        </li>
      </ul>
    </div>

    <div v-if="apiFailures.length" class="health-api">
      <p class="health-codes-title">Top 失败来源（按 api）</p>
      <ul class="health-api-list">
        <li v-for="stat in apiFailures" :key="stat.api" class="health-api-row">
          <span class="health-api-name ellipsis-1" :title="stat.api">{{ stat.api }}</span>
          <span class="health-api-meta">
            {{ stat.sites }} 站 · 累计 {{ stat.failures }} 次<template v-if="stat.disabled"> · 禁用 {{ stat.disabled }}</template>
          </span>
          <span
            v-if="stat.lastCode"
            class="health-api-code"
            :title="codeLabel(stat.lastCode) + (codeAction(stat.lastCode) ? '：' + codeAction(stat.lastCode) : '')"
          >{{ codeLabel(stat.lastCode) }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.health-empty {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.health-daily {
  margin-top: var(--space-3);
  padding-top: var(--space-2);
  border-top: 1px dashed var(--border-lighter);
}

.health-daily-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.health-daily-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: 12px;
  color: var(--text-muted);
}

.health-daily-day {
  flex-shrink: 0;
  width: 44px;
}

.health-daily-track {
  display: inline-flex;
  align-items: center;
  height: 8px;
  min-width: 2px;
  overflow: hidden;
  border-radius: 4px;
  background: var(--surface-3);
}

.health-daily-ok {
  height: 100%;
  background: var(--success, #22c55e);
}

.health-daily-fail {
  height: 100%;
  background: var(--warning, #f59e0b);
}

.health-daily-meta {
  flex-shrink: 0;
  color: var(--text-muted);
}

.health-events {
  margin-top: var(--space-3);
  padding-top: var(--space-2);
  border-top: 1px dashed var(--border-lighter);
}

.health-event-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.health-event-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.health-event-time {
  flex-shrink: 0;
  width: 64px;
}

.health-event-name {
  flex: 1;
  min-width: 0;
  color: var(--text);
}

.health-event-code {
  flex-shrink: 0;
  color: var(--text-muted);
}

.health-codes {
  margin-top: var(--space-3);
  padding-top: var(--space-2);
  border-top: 1px dashed var(--border-lighter);
}

.health-codes-title {
  margin: 0 0 var(--space-2);
  font-size: 12px;
  color: var(--text-muted);
}

.health-code-list {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin: 0;
  padding: 0;
  list-style: none;
}

.health-code-row {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  padding: 2px var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--surface-3);
  font-size: 12px;
}

.health-code-name {
  color: var(--text);
}

.health-code-meta {
  color: var(--text-muted);
}

/* 记录多时限制高度在内部滚动：标题与「全部重置」保持可见，页面不被撑得很长 */
.health-scroll {
  padding-right: var(--space-2);
}

.health-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin: 0;
  padding: 0;
  list-style: none;
}

.health-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.health-main {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.health-title-line {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.health-kind {
  flex-shrink: 0;
  padding: 0 var(--space-1);
  border-radius: var(--radius-sm);
  background: var(--surface-3);
  font-size: 11px;
  color: var(--text-muted);
}

.health-name {
  font-size: 13px;
  color: var(--text);
  min-width: 0;
}

.health-badge {
  padding: 3px;
  border-radius: var(--radius-sm);
  font-size: 11px;
  line-height: 1;
}

.health-badge.is-disabled {
  color: var(--danger);
  background: color-mix(in srgb, var(--danger) 12%, transparent);
}

.health-badge.is-ok {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 12%, transparent);
}

.health-meta {
  margin: 0;
  font-size: 11.5px;
  color: var(--text-muted);
}

.health-error {
  margin: 0;
  font-size: 11.5px;
  color: var(--danger);
}

.health-note {
  margin: var(--space-3) 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

.health-api {
  margin-top: var(--space-3);
  padding-top: var(--space-2);
  border-top: 1px dashed var(--border-lighter);
}

.health-api-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.health-api-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  font-size: 12px;
}

.health-api-name {
  flex: 1 1 auto;
  min-width: 0;
  color: var(--text);
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
}

.health-api-meta {
  flex-shrink: 0;
  color: var(--text-muted);
}

.health-api-code {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: var(--radius-sm);
  color: var(--warning, #f59e0b);
  background: color-mix(in srgb, var(--warning, #f59e0b) 12%, transparent);
}
</style>
