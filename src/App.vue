<!-- 应用根组件：只做全局初始化（主题 / 订阅配置），具体布局由 layouts/default 提供 -->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import DefaultLayout from '@/layouts/default/index.vue'
import { useConfigStore } from '@/store/config'
import { useUiStore } from '@/store/ui'
import { checkUpdate, updatesSupported } from '@/api/update'
import { isTauri } from '@/api'

const route = useRoute()
const router = useRouter()
const ui = useUiStore()
const config = useConfigStore()

/** 独立日志窗口：不套主布局（无侧栏 / 标题栏） */
const standalone = computed(() => route.name === 'log-window')

const MEDIA_EXT = new Set([
  'mp4', 'm4v', 'mkv', 'webm', 'mov', 'avi', 'flv', 'wmv',
  'ts', 'm2ts', 'mts', 'mpg', 'mpeg',
  'mp3', 'm4a', 'aac', 'flac', 'wav', 'ogg', 'opus',
])

/** 拖入文件 → 直接进本地播放页（走 PlayerPane，与下载页打开本地文件同一条链路） */
function handleDrop(paths: string[]) {
  const file = paths.find((path) => {
    const ext = path.split('.').pop()?.toLowerCase() ?? ''
    return MEDIA_EXT.has(ext)
  })
  if (!file) {
    ui.notify('只支持拖入视频或音频文件', 'info')
    return
  }
  const name = file.split(/[\\/]/).pop() ?? '本地播放'
  const title = name.replace(/\.[^.]+$/, '') || name
  void router.push({
    name: 'local-player',
    params: { id: 'dropped' },
    query: { path: file, title },
  })
}

let unlistenDrop: (() => void) | undefined

/** 启动后静默检查一次更新：有新版本只提示，不自动下载；失败静默（不打扰）。 */
function scheduleUpdateCheck() {
  if (!updatesSupported) return
  window.setTimeout(async () => {
    try {
      const info = await checkUpdate()
      if (info) ui.notify(`发现新版本 v${info.version}，可在「设置 → 关于」更新`, 'info')
    } catch {
      // 无网络 / 发布端未配置 latest.json 时保持静默
    }
  }, 8000)
}

onMounted(async () => {
  ui.init()
  await config.init()
  scheduleUpdateCheck()

  // Tauri 原生拖放（WebView 默认把文件拖放交给系统处理，浏览器里没有这个事件）
  if (isTauri) {
    try {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview')
      unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type !== 'drop') return
        handleDrop(event.payload.paths)
      })
    } catch (e) {
      console.warn('[VBox] 注册文件拖放失败：', e)
    }
  }
})

onBeforeUnmount(() => {
  unlistenDrop?.()
})
</script>

<template>
  <RouterView v-if="standalone" />
  <DefaultLayout v-else />
</template>
