<!-- 默认布局（AppShell）：自定义标题栏 + 侧栏 + 内容区。
     层级只用颜色区分：外壳（TitleBar / SideNav）一层底色，内容区一层更深底色；
     不做留白/阴影/分割线，也不放全局 TopBar。 -->
<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import { RouterView, useRoute, useRouter } from 'vue-router'
import * as api from '@/api'
import { navTarget } from '@/router/navMemory'
import TitleBar from './components/TitleBar.vue'
import SideNav from './components/SideNav.vue'
import NoticeToast from './components/NoticeToast.vue'

const router = useRouter()
const route = useRoute()

/** 焦点是否在可编辑元素内（此时不拦截 "/"，避免影响输入） */
function isEditableFocus(): boolean {
  const el = document.activeElement
  if (!(el instanceof HTMLElement)) return false
  const tag = el.tagName.toLowerCase()
  return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable
}

/** 全局快捷键：/ 搜索；Alt+1..4 一级菜单；Alt+, 设置（输入框内 Alt 组合键仍生效） */
function onKeydown(event: KeyboardEvent) {
  if (event.defaultPrevented || event.repeat) return

  if (event.key === '/' && !event.ctrlKey && !event.metaKey && !event.altKey) {
    if (isEditableFocus()) return
    event.preventDefault()
    void router.push('/vod/search')
    return
  }

  if (!event.altKey || event.ctrlKey || event.metaKey) return
  const currentNav = String(route.meta.nav ?? '')
  const linkTo = (nav: string, fallback: string): string =>
    currentNav === nav ? fallback : navTarget(nav, fallback)
  const targets: Record<string, string> = {
    Digit1: linkTo('vod', '/vod'),
    Digit2: linkTo('live', '/live'),
    Digit3: linkTo('history', '/history'),
    Digit4: linkTo('downloads', '/downloads'),
    Comma: linkTo('settings', '/settings'),
  }
  const target = targets[event.code]
  if (!target) return
  event.preventDefault()
  void router.push(target)
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  // 桌面端只做原生毛玻璃准备，默认不加 acrylic-on（亚克力默认关闭）；
  // 是否显示由设置 → 外观的开关控制（开/关只加/移除该类）
  void api.windowBackdrop()
})
onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <div class="app-shell">
    <TitleBar />
    <div class="app-body">
      <SideNav />
      <div class="app-main">
        <NoticeToast />
        <!-- 外壳不再提供滚动容器：滚动条下沉到每个页面（PageScroll），
             页面被 KeepAlive 缓存时滚动位置随页面 DOM 一起保留。
             不要在外层包 Transition：KeepAlive 切换缓存布局时会移动 DOM，
             Transition 的 leave 回调会在已移走的节点上执行，触发
             `el[leaveCbKey] is not a function` / parentNode 为 null 并卡死路由。 -->
        <main class="app-content">
          <RouterView v-slot="{ Component }">
            <KeepAlive
              :include="[
                'VodLayout',
                'LiveLayout',
                'HistoryLayout',
                'FavoritesLayout',
                'SettingsView',
                'DownloadsView',
              ]"
            >
              <component :is="Component" />
            </KeepAlive>
          </RouterView>
        </main>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 外壳统一底色；内容区仅用更深的底色区分层级 */
/* 播放时 .app-shell 用 evenodd clip-path 挖出播放器「视频洞」：
   - html/body/#app 透明（base.css），洞内不绘制任何内容 → 露出下层 libmpv 原生子窗口；
   - 洞外仍由 --shell-bg / --content-bg 正常绘制，窗口观感不变；
   - 变量由 usePlayer 以 CSS 像素 / 视口坐标写入 documentElement，默认 0（无洞）。 */
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  width: 100vw;
  overflow: hidden;
  background: var(--shell-bg);
  clip-path: polygon(
    evenodd,
    0 0,
    100% 0,
    100% 100%,
    0 100%,
    0 0,
    var(--vbox-hole-x, 0px) var(--vbox-hole-y, 0px),
    var(--vbox-hole-x, 0px) calc(var(--vbox-hole-y, 0px) + var(--vbox-hole-h, 0px)),
    calc(var(--vbox-hole-x, 0px) + var(--vbox-hole-w, 0px))
      calc(var(--vbox-hole-y, 0px) + var(--vbox-hole-h, 0px)),
    calc(var(--vbox-hole-x, 0px) + var(--vbox-hole-w, 0px)) var(--vbox-hole-y, 0px),
    var(--vbox-hole-x, 0px) var(--vbox-hole-y, 0px)
  );
}

.app-body {
  flex: 1;
  display: flex;
  min-height: 0;
  overflow: hidden;
  background: var(--shell-bg);
}

.app-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--shell-bg);
  transition: var(--transition-color);
}

.app-content {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow: hidden;
  background: var(--content-bg);
  border-top-left-radius: var(--radius-lg);
  border-bottom-left-radius: var(--radius-lg);
  transition: var(--transition-color);
}
</style>
