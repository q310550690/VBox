<!-- 本地文件播放页：只用于播放已下载到本地/下载目录里的文件（复用 PlayerPane） -->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import PlayerPane from '@/components/PlayerPane.vue'
import * as api from '@/api'

const route = useRoute()
const router = useRouter()

/** 本地文件绝对路径（query 已由 router 解码；Windows 反斜杠原样保留） */
const filePath = computed(() => String(route.query.path ?? ''))

/** 展示名（下载记录里的「剧名 · 集名」） */
const title = computed(() => {
  const value = route.query.title
  return typeof value === 'string' && value.trim() ? value : '本地播放'
})

const playerPane = ref<InstanceType<typeof PlayerPane> | null>(null)

async function mediaSource(): Promise<string> {
  const raw = filePath.value
  if (!raw) return raw
  try {
    // 走本地 HTTP（Range 按需读取）：moov 在尾部的 MP4 也能快速起播/拖动。
    return await api.localMediaUrl(raw)
  } catch {
    // 本地服务未就绪时退回绝对路径，保证仍能播放。
    return raw
  }
}

async function openLocal() {
  if (!filePath.value) return
  await playerPane.value?.open(await mediaSource(), {}, 0)
}

function goBack() {
  void router.push('/downloads')
}

onMounted(() => {
  void openLocal()
})

watch(filePath, () => {
  void openLocal()
})

onBeforeUnmount(() => {
  void playerPane.value?.close()
})
</script>

<template>
  <div class="page local-player-page">
    <header class="page-header">
      <div class="heading">
        <el-button circle aria-label="返回" @click="goBack">
          <template #icon><SolarIcon name="arrow-left" /></template>
        </el-button>
        <h1 class="page-title ellipsis-1" :title="title">{{ title }}</h1>
      </div>
      <span class="muted path" :title="filePath">{{ filePath }}</span>
    </header>

    <div class="pane-wrap">
      <PlayerPane
        ref="playerPane"
        :title="title"
        :series-name="title"
        :has-media="Boolean(filePath)"
        :loading="false"
      />
    </div>
  </div>
</template>

<style scoped>
.local-player-page {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  min-height: 0;
}

.heading {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.page-title {
  min-width: 0;
}

.path {
  max-width: 46%;
  overflow: hidden;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pane-wrap {
  position: relative;
  width: 100%;
  aspect-ratio: 16 / 9;
  max-height: calc(100vh - 180px);
  border-radius: var(--radius-lg);
  overflow: hidden;
  background: #000;
}
</style>
