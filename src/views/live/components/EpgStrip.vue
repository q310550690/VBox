<!-- 直播节目单条（frontend-design.md §6.5）：EPG 来自 live_epg（XMLTV），无数据时明确显示占位 -->
<script setup lang="ts">
import { computed } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { EpgProgram, LiveChannel } from '@/types'

const props = defineProps<{
  /** 当前频道（未选择为 null） */
  channel: LiveChannel | null
  /** 节目单（live_epg；时间已由 API 门面规范化为毫秒） */
  programs: EpgProgram[]
  loading?: boolean
  error?: string
}>()

const emit = defineEmits<{ (e: 'retry'): void }>()

/** 当前正在播出的节目下标（仅当 EPG 返回时间时计算，不伪造） */
const activeIndex = computed(() => {
  const now = Date.now()
  return props.programs.findIndex(
    (p) =>
      typeof p.start === 'number' &&
      typeof p.end === 'number' &&
      p.start <= now &&
      now < p.end,
  )
})

/** 时间戳 → HH:mm；缺失时显示占位而不用假时间 */
function formatClock(ts?: number | null): string {
  if (typeof ts !== 'number' || !Number.isFinite(ts)) return '--:--'
  const date = new Date(ts)
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}
</script>

<template>
  <section class="epg-strip" aria-label="节目单">
    <div class="epg-head">
      <SolarIcon class="epg-icon" name="clock-circle" aria-hidden="true" />
      <span class="epg-channel ellipsis-1">{{ channel ? channel.name : '未选择频道' }}</span>
      <span class="epg-caption">节目单</span>
      <el-button v-if="error" class="epg-retry" size="small" text type="primary" @click="emit('retry')">
        重试
      </el-button>
    </div>

    <div class="epg-body">
      <template v-if="loading">
        <span class="epg-skeleton" aria-hidden="true" />
        <span class="epg-skeleton short" aria-hidden="true" />
      </template>
      <p v-else-if="error" class="epg-error">节目单加载失败：{{ error }}</p>
      <template v-else-if="!programs.length">
        <span class="epg-placeholder">暂无节目单</span>
        <span class="epg-hint">该频道未配置 EPG，或 EPG 源没有返回当天节目（不编造时间）</span>
      </template>
      <ul v-else class="epg-list">
        <li
          v-for="(program, index) in programs"
          :key="`${program.title}-${index}`"
          class="epg-item"
          :class="{ current: index === activeIndex }"
        >
          <span class="epg-time">{{ formatClock(program.start) }} - {{ formatClock(program.end) }}</span>
          <span class="epg-title ellipsis-1">{{ program.title }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.epg-strip {
  margin-top: var(--space-4);
  padding: var(--space-3) var(--space-4);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}

.epg-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  color: var(--text-muted);
  font-size: 12px;
}

.epg-icon {
  color: var(--accent);
}

.epg-channel {
  max-width: 200px;
  color: var(--text);
  font-size: 13px;
  font-weight: 600;
}

.epg-caption {
  color: var(--text-faint);
}

.epg-retry {
  margin-left: auto;
}

.epg-body {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-wrap: wrap;
  margin-top: var(--space-2);
  min-height: 24px;
}

.epg-placeholder {
  font-size: 13px;
  font-weight: 600;
  color: var(--warning);
}

.epg-hint {
  font-size: 12px;
  color: var(--text-faint);
}

.epg-error {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
}

.epg-skeleton {
  width: 180px;
  height: 14px;
  border-radius: var(--radius-sm);
  background: var(--surface-3);
  animation: epg-pulse 1.2s ease-in-out infinite;
}

.epg-skeleton.short {
  width: 120px;
}

.epg-list {
  display: flex;
  gap: var(--space-3);
  margin: 0;
  padding: 0;
  list-style: none;
  overflow-x: auto;
  width: 100%;
  /* 横向滚动不显示原生滚动条（纵向滚动统一由 el-scrollbar 承载） */
  scrollbar-width: none;
}

.epg-list::-webkit-scrollbar {
  display: none;
}

.epg-item {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm);
  white-space: nowrap;
  color: var(--text-muted);
  font-size: 12px;
}

.epg-item.current {
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

.epg-time {
  font-variant-numeric: tabular-nums;
}

.epg-title {
  max-width: 200px;
}

@keyframes epg-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.55;
  }
}
</style>
