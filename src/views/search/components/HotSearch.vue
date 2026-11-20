<!-- 搜索页 · 热搜条目：豆瓣热门片名派生的 tag 标签云（可点击直接搜索） -->
<script setup lang="ts">
defineProps<{
  /** 热搜关键词（由 store 从豆瓣首页提取，失败时用兜底词） */
  items: string[]
  /** 正在拉取热搜数据 */
  loading?: boolean
}>()

const emit = defineEmits<{
  (e: 'pick', keyword: string): void
}>()

/** 热力配色：越靠前越红，用一条 --heat 变量驱动底色/文字/圆标，不参与 T3 标签体系 */
function heatColor(index: number): string {
  if (index < 3) return 'var(--el-color-danger)'
  if (index < 8) return 'var(--el-color-warning)'
  return 'var(--el-color-primary)'
}
</script>

<template>
  <section class="hot-panel panel" aria-labelledby="search-hot-title">
    <header class="hot-head">
      <h2 id="search-hot-title" class="hot-title">热搜</h2>
      <span v-if="loading" class="hot-loading">更新中…</span>
    </header>
    <el-scrollbar class="hot-scroll">
      <div class="hot-tags">
        <button
          v-for="(keyword, index) in items"
          :key="keyword"
          class="hot-tag"
          :class="{ 'is-top': index < 3 }"
          type="button"
          :style="{ '--heat': heatColor(index) }"
          @click="emit('pick', keyword)"
        >
          <span class="hot-rank">{{ index + 1 }}</span>
          {{ keyword }}
        </button>
        <span v-if="!items.length && !loading" class="hot-empty">暂无热搜数据</span>
      </div>
    </el-scrollbar>
  </section>
</template>

<style scoped>
.hot-panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  padding: var(--space-3);
}

.hot-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
  flex-shrink: 0;
}

.hot-title {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.hot-loading {
  font-size: 12px;
  color: var(--text-faint);
}

.hot-scroll {
  flex: 1;
  min-height: 0;
}

.hot-tags {
  display: flex;
  flex-wrap: wrap;
  align-content: flex-start;
  gap: var(--space-2);
}

/* 热力 chip：无描边，靠"染色底 + 提亮文字"表达热度，越靠前越暖越实 */
.hot-tag {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  height: 32px;
  padding: 0 13px;
  overflow: hidden;
  border: 0;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--heat) 16%, var(--el-bg-color-page));
  color: color-mix(in srgb, var(--heat) 82%, var(--el-text-color-primary));
  font: inherit;
  font-size: 13px;
  font-weight: 500;
  white-space: nowrap;
  text-overflow: ellipsis;
  cursor: pointer;
  transition:
    background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease);
}

/* 前三名：底色更重，字回归正文色，热度靠面积与浓度顶住 */
.hot-tag.is-top {
  background: color-mix(in srgb, var(--heat) 30%, var(--el-bg-color-page));
  color: var(--el-text-color-primary);
  font-weight: 600;
}

.hot-tag.is-top .hot-rank {
  color: color-mix(in srgb, var(--heat) 70%, var(--el-text-color-primary));
}

.hot-tag:hover {
  background: color-mix(in srgb, var(--heat) 26%, var(--el-bg-color-page));
  color: color-mix(in srgb, var(--heat) 92%, var(--el-text-color-primary));
}

.hot-tag.is-top:hover {
  background: color-mix(in srgb, var(--heat) 40%, var(--el-bg-color-page));
  color: var(--el-text-color-primary);
}

.hot-tag:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--heat) 45%, var(--el-bg-color-page));
}

.hot-rank {
  font-weight: 700;
  opacity: 0.85;
  font-variant-numeric: tabular-nums;
}

.hot-empty {
  font-size: 12px;
  color: var(--text-faint);
}
</style>
