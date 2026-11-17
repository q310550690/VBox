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

/** 前三名用醒目颜色，其余用主色浅底（不使用白底 plain / small） */
function tagType(index: number): 'danger' | 'warning' | 'primary' {
  if (index < 3) return 'danger'
  if (index < 8) return 'warning'
  return 'primary'
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
        <el-tag
          v-for="(keyword, index) in items"
          :key="keyword"
          class="hot-tag"
          :type="tagType(index)"
          effect="light"
          @click="emit('pick', keyword)"
        >
          <span class="hot-rank">{{ index + 1 }}</span>
          {{ keyword }}
        </el-tag>
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

.hot-tag {
  cursor: pointer;
  max-width: 100%;
  font-size: 13px;
}

.hot-tag :deep(.el-tag__content) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hot-rank {
  margin-right: 4px;
  font-weight: 600;
  opacity: 0.75;
  font-variant-numeric: tabular-nums;
}

.hot-empty {
  font-size: 12px;
  color: var(--text-faint);
}
</style>
