<!-- 自定义标题栏：隐藏 Tauri 原生标题栏（tauri.conf.json → window.decorations=false）后，
     提供拖拽区域与 最小化 / 最大化(还原) / 关闭 三个窗口按钮。图标全部来自 Solar Linear（src/icons/solar.ts）。 -->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { getCurrentWindow } from '@tauri-apps/api/window'
import SolarIcon from '@/components/SolarIcon.vue'
import { useUiStore } from '@/store/ui'
import { isTauri } from '@/api'

const route = useRoute()

const ui = useUiStore()

/** 标题栏显示的当前页面名（路由 meta.title） */
const pageTitle = computed<string>(() => String(route.meta.title ?? ''))

// 浏览器预览（非 Tauri）下 getCurrentWindow() 会因缺少 __TAURI_INTERNALS__ 抛错，
// 因此必须延迟到确认运行在 Tauri 内再获取窗口对象。
const appWindow = isTauri ? getCurrentWindow() : null
const isMaximized = ref(false)

async function refreshMaximized() {
  if (!appWindow) return
  try {
    isMaximized.value = await appWindow.isMaximized()
  } catch {
    /* 非 Tauri 环境（浏览器预览）忽略 */
  }
}

async function onMinimize() {
  if (!appWindow) return
  try {
    await appWindow.minimize()
  } catch {
    /* 忽略 */
  }
}

async function onToggleMaximize() {
  if (!appWindow) return
  try {
    await appWindow.toggleMaximize()
    await refreshMaximized()
  } catch {
    /* 忽略 */
  }
}

async function onClose() {
  if (!appWindow) return
  try {
    await appWindow.close()
  } catch {
    /* 忽略 */
  }
}

let unlisten: (() => void) | null = null

onMounted(async () => {
  await refreshMaximized()
  if (!appWindow) return
  try {
    unlisten = await appWindow.onResized(() => refreshMaximized())
  } catch {
    /* 忽略 */
  }
})

onUnmounted(() => {
  unlisten?.()
})
</script>

<template>
  <header
    class="titlebar"
    :class="{ 'is-collapsed': ui.sidebarCollapsed }"
    data-tauri-drag-region
    @dblclick="onToggleMaximize"
  >
    <div class="tb-left" data-tauri-drag-region>
      <button
        class="tb-toggle"
        type="button"
        :title="ui.sidebarCollapsed ? '展开侧栏' : '折叠侧栏'"
        :aria-label="ui.sidebarCollapsed ? '展开侧栏' : '折叠侧栏'"
        :aria-expanded="!ui.sidebarCollapsed"
        @click.stop="ui.toggleSidebar()"
      >
        <SolarIcon
          :size="16"
          :name="ui.sidebarCollapsed ? 'panel-left-open' : 'panel-left-close'"
        />
      </button>
      <div class="tb-logo-area" v-if="ui.sidebarCollapsed">
        <img class="tb-logo" src="/logo.png" alt="VBox" draggable="false" data-tauri-drag-region />
        <span class="tb-name">VBox</span>
      </div>
      <span v-if="pageTitle" class="tb-page" data-tauri-drag-region>{{ pageTitle }}</span>
    </div>

    <div class="tb-drag" data-tauri-drag-region />

    <div class="tb-controls">
      <button class="tb-btn" type="button" title="最小化" aria-label="最小化" @click.stop="onMinimize">
        <SolarIcon :size="14" name="minus" />
      </button>
      <button
        class="tb-btn"
        type="button"
        :title="isMaximized ? '还原' : '最大化'"
        :aria-label="isMaximized ? '还原' : '最大化'"
        @click.stop="onToggleMaximize"
      >
        <SolarIcon v-if="isMaximized" :size="13" name="minimize-square" />
        <SolarIcon v-else :size="13" name="maximize-square" />
      </button>
      <button
        class="tb-btn tb-btn-close"
        type="button"
        title="关闭"
        aria-label="关闭"
        @click.stop="onClose"
      >
        <SolarIcon :size="14" name="close" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  height: var(--titlebar-h);
  flex-shrink: 0;
  display: flex;
  align-items: center;
  background: var(--shell-bg);
  user-select: none;
  transition: var(--transition-color);
  /* 外壳固定高于内容区（含 player-overlay），低于全局消息层 */
  position: relative;
  z-index: 100;
}

.tb-left {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 0 var(--space-4) 0 13px;
  height: 100%;
  min-width: 0;
}

.tb-toggle {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 28px;
  height: 28px;
  margin-right: -4px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: var(--transition-color);
}

.tb-toggle:hover {
  color: var(--text-strong);
}

/** 折叠/展开时把折叠按钮图标对到主导航图标那一列（展开 27px、折叠 32px 中心） */
.titlebar.is-collapsed .tb-left {
  padding-left: 18px;
}
.tb-logo-area{
  display: flex;
  align-items: center;
}
.tb-logo {
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  object-fit: contain;
  margin-right: 2px;
}

.tb-name,.tb-page {
  color: var(--text-regular);
  font-size: 12px;
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tb-drag {
  flex: 1;
  height: 100%;
  min-width: var(--space-4);
}

.tb-controls {
  display: flex;
  height: 100%;
  flex-shrink: 0;
}

.tb-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 46px;
  height: 100%;
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: var(--transition-color);
}

.tb-btn:hover {
  background: var(--accent-weak);
  color: var(--text);
}

.tb-btn-close:hover {
  background: var(--danger-solid);
  color: var(--accent-on);
}

.tb-btn:active {
  background: var(--accent-weak-hover);
}
</style>
