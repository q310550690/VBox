<!-- 侧栏导航（frontend-design.md §2 / §5.1）
     - 品牌行：/logo.png + VBox（折叠时只留 logo）
     - 分组菜单：媒体 / 我的 / 插件（插件组由 pluginItems 驱动，接入插件后自动出现）
     - 底部：纯文字信息块（名称 / 简介 / 版本 / 协议 / 版权）+ 横排小图标（主题、设置）
     - 展开 / 折叠由标题栏最左侧按钮控制（ui.sidebarCollapsed），折叠时还原为 64px 图标栏 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import { navTarget } from '@/router/navMemory'
import ThemeToggle from './ThemeToggle.vue'
import { useUiStore } from '@/store/ui'
import { appVersion, checkUpdate, updatesSupported } from '@/api/update'
import { formatError } from '@/api'

interface NavItem {
  /** 对应 route.meta.nav（点播详情、搜索页都归在 vod 下） */
  nav: string
  to: string
  label: string
  icon: SolarIconName
}

interface NavGroup {
  key: string
  title: string
  items: NavItem[]
}

const ui = useUiStore()
const route = useRoute()

/** 展开 / 折叠由 store 统一管理（标题栏按钮切换，窗口 < 960px 自动折叠） */
const collapsed = computed<boolean>(() => ui.sidebarCollapsed)

/** 版本：桌面端读 Tauri 运行时版本，浏览器预览用构建期常量兜底；协议固定 */
const APP_VERSION = ref(__APP_VERSION__)
const APP_LICENSE = 'AGPL-3.0-only'

onMounted(async () => {
  try {
    APP_VERSION.value = await appVersion()
  } catch {
    // 浏览器预览：保留构建期版本
  }
})

/** 插件入口：插件加载后往这里 push 即可出现在「插件」分组，无需改模板 */
const pluginItems: NavItem[] = []

const checkingUpdate = ref(false)

/** 检查更新（桌面端真检查；浏览器预览直接告知不支持） */
async function onCheckUpdate() {
  if (checkingUpdate.value) return
  if (!updatesSupported) {
    ui.notify('浏览器预览模式不支持检查更新（需桌面端）', 'info')
    return
  }
  checkingUpdate.value = true
  try {
    const info = await checkUpdate()
    if (info?.version) ui.notify(`发现新版本 v${info.version}`, 'info')
    else ui.notify('已是最新版本', 'success')
  } catch (err) {
    ui.notify(`检查更新失败：${formatError(err)}`, 'error')
  } finally {
    checkingUpdate.value = false
  }
}

/** 分享：复制 GitHub 仓库地址到剪贴板 */
const REPO_URL = 'https://github.com/q310550690/VBox'

async function onShare() {
  try {
    await navigator.clipboard.writeText(REPO_URL)
    ui.notify('GitHub 仓库地址已复制', 'success')
  } catch {
    ui.notify('复制失败，请检查剪贴板权限', 'error')
  }
}

const navGroups: NavGroup[] = [
  {
    key: 'media',
    title: '媒体',
    items: [
      { nav: 'vod', to: '/vod', label: '点播', icon: 'clapperboard' },
      { nav: 'cloud', to: '/cloud', label: '云盘', icon: 'cloud-storage' },
      { nav: 'live', to: '/live', label: '直播', icon: 'videocamera' },
    ],
  },
  {
    key: 'mine',
    title: '我的',
    items: [
      { nav: 'favorites', to: '/favorites', label: '收藏', icon: 'star' },
      { nav: 'history', to: '/history', label: '历史记录', icon: 'clock-circle' },
      { nav: 'downloads', to: '/downloads', label: '下载', icon: 'download' },
    ],
  },
  { key: 'plugin', title: '插件', items: pluginItems },
]

/** 空分组（例如还没装插件时的「插件」）不渲染，也不留标题 */
const visibleGroups = computed<NavGroup[]>(() => navGroups.filter((g) => g.items.length > 0))

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
      <nav class="nav" aria-label="一级菜单">
        <section
          v-for="group in visibleGroups"
          :key="group.key"
          class="nav-group"
        >
          <p class="nav-group-title">{{ group.title }}</p>
          <RouterLink
            v-for="item in group.items"
            :key="item.nav"
            class="nav-item"
            :class="{ 'is-active': isActive(item.nav) }"
            :to="targetFor(item.nav, item.to)"
            :aria-label="item.label"
            :aria-current="isActive(item.nav) ? 'page' : undefined"
            :title="item.label"
          >
            <SolarIcon class="nav-icon" :size="18" :name="item.icon" />
            <span class="nav-label">{{ item.label }}</span>
          </RouterLink>
        </section>
      </nav>

      <div class="spacer" />

      <div class="bottom">
        <div class="brand" aria-label="VBox">
          <div class="brand-top">
            <img class="brand-logo" src="/logo.png" alt="VBox" draggable="false" />
            <span class="brand-name">VBox</span>
          </div>
          <span class="brand-sub">一匣藏山海，万象皆入画</span>
        </div>
        <div class="meta" aria-label="版本与协议">
          <div class="meta-line">Ver {{ APP_VERSION }} · {{ APP_LICENSE }}</div>
          <div class="meta-line">© {{ new Date().getFullYear() }} 诗小柒</div>
        </div>
        <div class="rail">
          <button
            class="rail-item"
            type="button"
            :disabled="checkingUpdate"
            aria-label="检查更新"
            title="检查更新"
            @click="onCheckUpdate"
          >
            <SolarIcon :size="18" name="refresh-circle" />
          </button>
          <button
            class="rail-item"
            type="button"
            aria-label="复制仓库地址"
            title="分享 · 复制仓库地址"
            @click="onShare"
          >
            <SolarIcon :size="18" name="link-round" />
          </button>
          <ThemeToggle
            :mode="ui.mode"
            :resolved="ui.resolvedMode"
            collapsed
            @toggle="ui.cycleMode()"
          />
          <RouterLink
            class="rail-item"
            :class="{ 'is-active': isActive('settings') }"
            :to="targetFor('settings', '/settings')"
            aria-label="设置"
            title="设置"
          >
            <SolarIcon :size="18" name="settings" />
          </RouterLink>
        </div>

      </div>
    </el-scrollbar>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  width: 160px;
  overflow: hidden;
  background: var(--shell-bg);
  user-select: none;
  transition: width 0.18s var(--ease), background-color 0.18s ease;
  position: relative;
  z-index: 100;
}

/* —— 品牌行 —— */
.brand {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  gap: 4px;
  padding-bottom: 4px;
}

/* logo 与名称同一行、垂直居中；标语另起一行，从 logo 左缘起排 */
.brand-top {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 30px;
}

.brand-logo {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  object-fit: contain;
}

.brand-name {
  font-size: 15px;
  font-weight: 650;
  letter-spacing: 0.2px;
  color: var(--text-strong);
  line-height: 1.3;
}

.brand-sub {
  overflow: hidden;
  font-size: 11px;
  color: var(--text-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: justify;
  text-align-last: justify;
}

.sidebar-scroll {
  flex: 1;
  min-height: 0;
}

.sidebar :deep(.el-scrollbar__view) {
  display: flex;
  flex-direction: column;
  min-height: 100%;
  box-sizing: border-box;
  padding: 4px 8px 8px;
}

/* —— 分组 —— */
.nav {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}

.nav-group {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.nav-group + .nav-group {
  margin-top: 6px;
}

.nav-group-title {
  margin: 6px 0 3px;
  padding: 0 10px;
  font-size: 11px;
  font-weight: 500;
  letter-spacing: 0.04em;
  color: var(--text-muted);
}

.nav-item {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: 10px;
  height: 34px;
  padding: 0 10px;
  border-radius: var(--radius-md);
  color: var(--text-regular);
  text-decoration: none;
  white-space: nowrap;
  transition: background-color 0.16s var(--ease), color 0.16s var(--ease);
}

.nav-item:hover {
  background: var(--surface-2);
  color: var(--text-strong);
}

.nav-item.is-active {
  background: var(--surface-2);
  color: var(--text-strong);
  font-weight: 600;
}

.nav-item.is-active .nav-icon {
  color: var(--accent);
}

.nav-item.is-active .nav-label {
  color: var(--accent);
}

.nav-icon {
  flex-shrink: 0;
}

.nav-label {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
}

.spacer {
  flex: 1;
  min-height: var(--space-4);
}

/* —— 底部：文字信息 + 横排小图标 —— */
.bottom {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  padding: 0 8px;
}

/* —— 版本 / 协议 / 版权（纯文字，无控件感） —— */
.meta {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  gap: 4px;
  padding-bottom: 4px;
}

.meta-line {
  margin: 0;
  font-size: 11px;
  letter-spacing: 0.2px;
  color: var(--text-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rail {
  display: flex;
  align-items: center;
  gap: 2px;
  /* 抵消 el-scrollbar__view 的 8px 内边距，让第一个图标与导航图标同一列（中心 27px） */
  margin-left: -6px;
  margin-right: -6px;
}

.rail-item {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 34px;
  height: 34px;
  padding: 0;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  font: inherit;
  color: var(--text-regular);
  cursor: pointer;
  text-decoration: none;
  transition: background-color 0.16s var(--ease), color 0.16s var(--ease);
}

.rail-item:disabled {
  opacity: 0.5;
  cursor: default;
}

.rail-item:hover {
  color: var(--accent);
}

.rail-item.is-active {
  color: var(--accent);
}

/* —— 折叠：还原为 64px 图标栏 —— */
.sidebar.is-collapsed {
  width: 64px;
}

.is-collapsed .brand {
  display: none;
}

.is-collapsed .meta {
  display: none;
}

.is-collapsed .brand-sub,
.is-collapsed .nav-group-title,
.is-collapsed .nav-label {
  display: none;
}

.is-collapsed .sidebar :deep(.el-scrollbar__view) {
  align-items: center;
  padding: 4px 0 8px;
}

.is-collapsed .nav-group,
.is-collapsed .nav {
  align-items: center;
}

.is-collapsed .nav-group {
  /* 折叠后与旧图标栏一致：40px 方块、间距 8（分组标题隐藏，间距统一） */
  gap: 8px;
}

.is-collapsed .nav-group + .nav-group {
  margin-top: 8px;
}

.is-collapsed .nav-item {
  justify-content: center;
  gap: 0;
  width: 40px;
  height: 40px;
  padding: 0;
  border-radius: 10px;
}

.is-collapsed .bottom {
  align-items: center;
  padding: 0;
}

.is-collapsed .rail {
  flex-direction: column;
  gap: 8px;
  margin: 8px 0 0;
}

/* 折叠态与上面的导航方块同规矩：40×40，移入 / 选中都铺 --surface-2，图标转 --accent */
.is-collapsed .rail-item,
.is-collapsed .rail :deep(.theme-toggle) {
  width: 40px;
  height: 40px;
  border-radius: 10px;
}

.is-collapsed .rail-item:hover,
.is-collapsed .rail-item.is-active,
.is-collapsed .rail :deep(.theme-toggle:hover),
.is-collapsed .rail :deep(.theme-toggle:active) {
  background: var(--surface-2);
  color: var(--accent);
}
</style>
