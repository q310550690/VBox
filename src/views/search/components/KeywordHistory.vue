<!-- 搜索页 · 搜索历史：tag 标签（非 small / 非白底），可点击回填、可单项删除、可清空 -->
<script setup lang="ts">
defineProps<{
  /** 历史关键词（由 useSearchStore 持久化，最多 20 条） */
  items: string[]
}>()

const emit = defineEmits<{
  (e: 'pick', keyword: string): void
  (e: 'remove', keyword: string): void
  (e: 'clear'): void
}>()
</script>

<template>
  <section class="keyword-history panel" aria-labelledby="search-history-title">
    <header class="head">
      <h2 id="search-history-title" class="label">搜索历史</h2>
      <el-button
        v-if="items.length"
        link
        class="clear"
        @click="emit('clear')"
      >
        清空
      </el-button>
    </header>

    <el-scrollbar class="history-scroll">
      <div v-if="items.length" class="chips">
        <el-tag
          v-for="keyword in items"
          :key="keyword"
          class="chip"
          type="primary"
          effect="light"
          closable
          @click="emit('pick', keyword)"
          @close="emit('remove', keyword)"
        >
          {{ keyword }}
        </el-tag>
      </div>
      <p v-else class="empty">暂无搜索历史，搜索后会记录在这里</p>
    </el-scrollbar>
  </section>
</template>

<style scoped>
.keyword-history {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
  min-height: 0;
  padding: var(--space-3);
}

.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.label {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.clear {
  color: var(--text-muted);
}

.history-scroll {
  flex: 1;
  min-height: 0;
}

.chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  align-items: flex-start;
}

.chip {
  cursor: pointer;
  max-width: 100%;
  font-size: 13px;
}

.empty {
  margin: 0;
  font-size: 12px;
  color: var(--text-faint);
}
</style>
