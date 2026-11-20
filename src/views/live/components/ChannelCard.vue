<!-- 直播频道卡片（frontend-design.md §6.5）：首字/图标占位 + 频道名 + 当前节目 -->
<script setup lang="ts">
import { computed } from 'vue'
import type { LiveChannel } from '@/types'

const props = defineProps<{
  /** 频道数据（来自 api.liveGroups(index) 返回的分组频道） */
  channel: LiveChannel
}>()

const emit = defineEmits<{ (e: 'select', channel: LiveChannel): void }>()

/** 无台标时用频道名首字占位（不伪造图片） */
const initial = computed(() => props.channel.name.trim().charAt(0) || '频')
</script>

<template>
  <button
    type="button"
    class="channel-card"
    :aria-label="`观看频道：${channel.name}`"
    @click="emit('select', channel)"
  >
    <span class="channel-logo" aria-hidden="true">
      <img v-if="channel.logo" class="logo-img" :src="channel.logo" :alt="channel.name" loading="lazy" />
      <template v-else>{{ initial }}</template>
    </span>
    <span class="channel-info">
      <span class="channel-name ellipsis-1">{{ channel.name }}</span>
      <span class="channel-now ellipsis-1">
        {{ channel.nowPlaying || '当前节目待接入' }}
      </span>
    </span>
  </button>
</template>

<style scoped>
.channel-card {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: 11px 14px;
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-card);
  box-shadow: var(--shadow-1);
  color: var(--text);
  text-align: left;
  cursor: pointer;
  transition:
    border-color var(--dur-base) var(--ease),
    box-shadow var(--dur-base) var(--ease),
    transform var(--dur-base) var(--ease);
}

.channel-card:hover {
  box-shadow: var(--shadow-2);
  transform: translateY(-1px);
}

.channel-card:active {
  transform: scale(0.99);
}

.channel-logo {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 34px;
  width: 34px;
  height: 34px;
  border-radius: var(--radius-md);
  background: var(--surface-2);
  color: var(--text-muted);
  font-size: 15px;
  font-weight: 600;
  overflow: hidden;
}

.logo-img {
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: var(--surface);
}

.channel-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}

.channel-name {
  font-size: 14px;
  font-weight: 600;
}

.channel-now {
  font-size: 12px;
  color: var(--text-muted);
}

@media (prefers-reduced-motion: reduce) {
  .channel-card {
    transition: var(--transition-color);
  }
  .channel-card:active {
    transform: none;
  }
}
</style>
