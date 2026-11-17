<!-- 侧栏导航（frontend-design.md §2 / §5.1）
     - 顶部 Logo（展开 VBox / 折叠 V）；5 项一级菜单（点播 / 直播 / 收藏 / 历史记录 / 下载）
     - 弹性空白后是底部区：ThemeToggle（主题切换）→ 1px 分隔线 → 设置（贴底）
     - 折叠状态统一取自 useUiStore().sidebarCollapsed（窗口 < 960px 由 store 自动折叠），
       折叠时仅图标 + tooltip，不在这里重复监听 resize -->
<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import { navTarget } from '@/router/navMemory'
import ThemeToggle from './ThemeToggle.vue'
import { useUiStore } from '@/store/ui'

interface NavItem {
  /** 对应 route.meta.nav（点播详情、搜索页都归在 vod 下） */
  nav: string
  to: string
  label: string
  icon: SolarIconName
}

const ui = useUiStore()
const route = useRoute()

/** 侧栏固定为图标栏：不显示文字，仅 hover tooltip */
const collapsed = true as const

const navItems: NavItem[] = [
  { nav: 'vod', to: '/vod', label: '点播', icon: 'clapperboard' },
  { nav: 'live', to: '/live', label: '直播', icon: 'videocamera' },
  { nav: 'favorites', to: '/favorites', label: '收藏', icon: 'star' },
  { nav: 'history', to: '/history', label: '历史记录', icon: 'clock-circle' },
  { nav: 'downloads', to: '/downloads', label: '下载', icon: 'download' },
]

/** 当前路由所属一级菜单（点播/直播的子页面同样归到对应菜单） */
const activeNav = computed<string>(() => String(route.meta.nav ?? ''))

function isActive(nav: string): boolean {
  return activeNav.value === nav
}

/**
 * 点一级菜单时：
 * - 已在该菜单内：回该菜单默认入口（例如详情页点「点播」回点播首页）
 * - 不在该菜单内：回到该菜单上次停留的页面（例如从设置点「点播」回详情页）
 */
function targetFor(nav: string, fallback: string): string {
  return isActive(nav) ? fallback : navTarget(nav, fallback)
}
</script>

<template>
  <aside
    class="sidebar"
    :class="{ 'is-collapsed': collapsed }"
    aria-label="应用导航"
  >
    <el-scrollbar class="sidebar-scroll">
      <!-- 一级菜单（图标栏）：不显示文字，也不显示悬浮提示 -->
      <nav
        class="nav"
        aria-label="一级菜单"
      >
        <RouterLink
          v-for="item in navItems"
          :key="item.nav"
          class="nav-item"
          :class="{ 'is-active': isActive(item.nav) }"
          :to="targetFor(item.nav, item.to)"
          :aria-label="item.label"
          :aria-current="isActive(item.nav) ? 'page' : undefined"
        >
          <SolarIcon
            class="nav-icon"
            :size="18"
            :name="item.icon"
          />
        </RouterLink>
      </nav>

      <!-- 弹性空白：把底部区推到最下方 -->
      <div class="spacer" />

      <!-- 底部区：主题切换（在设置上方）；不用分隔线，靠间距区分 -->
      <div class="bottom">
        <ThemeToggle
          :mode="ui.mode"
          :resolved="ui.resolvedMode"
          :collapsed="collapsed"
          @toggle="ui.cycleMode()"
        />

        <RouterLink
          class="nav-item settings-item"
          :class="{ 'is-active': isActive('settings') }"
          :to="targetFor('settings', '/settings')"
          aria-label="设置"
          :aria-current="isActive('settings') ? 'page' : undefined"
        >
          <SolarIcon
            class="nav-icon"
            :size="18"
            name="settings"
          />
        </RouterLink>
      </div>
    </el-scrollbar>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  /* width: var(--sidebar-w-collapsed); */
  overflow: hidden;
  background: var(--shell-bg);
  user-select: none;
  transition: width 0.18s ease, background-color 0.18s ease;
  /* 与 TitleBar 同层：高于内容区与 player-overlay，低于全局消息层 */
  position: relative;
  z-index: 100;
}

.sidebar-scroll {
  height: 100%;
}

.sidebar :deep(.el-scrollbar__view) {
  display: flex;
  flex-direction: column;
  align-items: center;
  min-height: 100%;
  box-sizing: border-box;
  padding: var(--space-2);
}

.sidebar.is-collapsed {
  /* width: var(--sidebar-w-collapsed); */
}

/* —— 一级菜单 —— */
.nav {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  flex-shrink: 0;
}

.nav-item {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 40px;
  height: 40px;
  padding: 0;
  border-radius: 10px;
  color: var(--text-regular);
  text-decoration: none;
  white-space: nowrap;
  transition: var(--transition-color);
}

.nav-item:hover {
  background: var(--surface-2);
  color: var(--text);
}

.nav-item.is-active {
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

.nav-item.is-active:hover {
  background: var(--accent-weak-hover);
}

.nav-icon {
  flex-shrink: 0;
}

.nav-label {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
}

.is-collapsed .nav-item {
  justify-content: center;
  gap: 0;
  padding: 0;
}

/* —— 底部区 —— */
.spacer {
  flex: 1;
  min-height: var(--space-4);
}

.bottom {
  display: flex;
  flex-direction: column;
  align-items: center;
  flex-shrink: 0;
  gap: var(--space-1);
}

/* 设置与主题切换同为 40px 方形行高 */
.settings-item {
  flex-shrink: 0;
  height: 40px;
}
</style>
