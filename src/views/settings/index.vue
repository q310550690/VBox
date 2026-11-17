<!-- 设置页：订阅 / 运行环境 / 播放与缓存 / 搜索与历史 / 网络与日志 / 外观 / 关于（分组卡片 + 吸顶分区导航） -->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import StatusTabs from '@/components/StatusTabs.vue'
import PageScroll from '@/components/PageScroll.vue'
import AboutSection from './components/AboutSection.vue'
import AppearanceSection from './components/AppearanceSection.vue'
import DownloadSection from './components/DownloadSection.vue'
import FfmpegSection from './components/FfmpegSection.vue'
import HistorySection from './components/HistorySection.vue'
import LogsSection from './components/LogsSection.vue'
import NetworkSection from './components/NetworkSection.vue'
import NodeSection from './components/NodeSection.vue'
import PlaybackCacheSection from './components/PlaybackCacheSection.vue'
import PlaybackSection from './components/PlaybackSection.vue'
import PluginSection from './components/PluginSection.vue'
import PythonSection from './components/PythonSection.vue'
import SearchSection from './components/SearchSection.vue'
import SourceHealthSection from './components/SourceHealthSection.vue'
import SubscriptionSection from './components/SubscriptionSection.vue'

/** 设置页共享样式：各 section 完全一致的重复规则统一放这里（作用域 .settings-page） */
import './sections.css'

defineOptions({ name: 'SettingsView' })

/** 分区锚点（切换 tab 滚动到对应卡片；滚动容器是 .app-content，scrollIntoView 会选最近可滚动祖先） */
const SECTION_TABS: { value: string; label: string }[] = [
  { value: 'anchor-subscription', label: '订阅管理' },
  { value: 'anchor-runtime', label: '运行环境' },
  { value: 'anchor-playback', label: '播放与缓存' },
  { value: 'anchor-plugins', label: '插件' },
  { value: 'anchor-search', label: '搜索与历史' },
  { value: 'anchor-network', label: '网络与日志' },
  { value: 'anchor-appearance', label: '外观' },
  { value: 'anchor-about', label: '关于' },
]

/** 当前高亮的分区 tab */
const activeSection = ref<string>(SECTION_TABS[0].value)

function onSectionChange(anchor: string) {
  activeSection.value = anchor
  document.getElementById(anchor)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

/* 滚动联动：手动滚动时高亮当前所在分区（滚动容器是设置页自己的 PageScroll） */
let scrollEl: HTMLElement | null = null

function syncActiveSection() {
  if (!scrollEl) return
  const line = scrollEl.getBoundingClientRect().top + 120
  let current = SECTION_TABS[0].value
  for (const item of SECTION_TABS) {
    const el = document.getElementById(item.value)
    if (el && el.getBoundingClientRect().top <= line) current = item.value
  }
  activeSection.value = current
}

onMounted(() => {
  scrollEl =
    document
      .querySelector<HTMLElement>('.settings-page')
      ?.closest<HTMLElement>('.page-scroll')
      ?.querySelector<HTMLElement>('.el-scrollbar__wrap') ?? null
  scrollEl?.addEventListener('scroll', syncActiveSection, { passive: true })
})

onBeforeUnmount(() => {
  scrollEl?.removeEventListener('scroll', syncActiveSection)
  scrollEl = null
})
</script>

<template>
  <PageScroll>
    <div class="page settings-page">
    <!-- 吸顶分区导航：与下载页 / 历史页共用 StatusTabs 药丸 tab -->
    <div class="settings-nav">
      <StatusTabs
        :model-value="activeSection"
        :items="SECTION_TABS"
        aria-label="设置分区导航"
        wrap
        @update:model-value="onSectionChange"
      />
    </div>

    <!-- 订阅管理 + 源健康（站点/线路失败计数与重置，紧挨订阅便于排查） -->
    <div
      id="anchor-subscription"
      class="settings-card panel settings-card--group"
    >
      <SubscriptionSection />
      <SourceHealthSection />
    </div>

    <!-- 运行环境：Python / Node / FFmpeg 合并 -->
    <div
      id="anchor-runtime"
      class="settings-card panel settings-card--group"
    >
      <h2 class="group-title">运行环境</h2>
      <PythonSection />
      <NodeSection />
      <FfmpegSection />
    </div>

    <!-- 播放与缓存：播放 / 下载与缓存 / 播放缓存 合并 -->
    <div
      id="anchor-playback"
      class="settings-card panel settings-card--group"
    >
      <h2 class="group-title">播放与缓存</h2>
      <PlaybackSection />
      <DownloadSection />
      <PlaybackCacheSection />
    </div>

    <!-- 插件：GLSL 着色器包 / lavfi 滤镜包 -->
    <div
      id="anchor-plugins"
      class="settings-card panel"
    >
      <PluginSection />
    </div>

    <!-- 搜索与历史：搜索 / 历史记录 合并 -->
    <div
      id="anchor-search"
      class="settings-card panel settings-card--group"
    >
      <h2 class="group-title">搜索与历史</h2>
      <SearchSection />
      <HistorySection />
    </div>

    <!-- 网络与日志：网络与抓包 / 运行日志 合并 -->
    <div
      id="anchor-network"
      class="settings-card panel settings-card--group"
    >
      <h2 class="group-title">网络与日志</h2>
      <NetworkSection />
      <LogsSection />
    </div>

    <div
      id="anchor-appearance"
      class="settings-card "
    >
      <AppearanceSection />
    </div>
    <div
      id="anchor-about"
      class="settings-card "
    >
      <AboutSection />
    </div>
    </div>
  </PageScroll>
</template>

<style scoped>
/* 吸顶导航容器：tab 样式由 StatusTabs 组件提供 */
.settings-nav {
  position: sticky;
  top: 0;
  z-index: 999;
  width: fit-content;
  max-width: 100%;
}

.settings-card {
  /* 吸顶导航约 48px + 页面留白，避免点 tab 后卡片标题被导航盖住 */
  scroll-margin-top: 64px;
}

/* 分组卡片：一个 tab 对应一个卡片、内部多个子分区 */
.group-title {
  margin: 0 0 var(--space-3);
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}

/* 子分区之间用细分割线隔开，标题降一级 */
.settings-card--group :deep(.settings-section) + .settings-section {
  margin-top: var(--space-4);
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-lighter);
}


.settings-card--group :deep(.section-title) {
  font-size: 14px;
  line-height: 1;
}

/* 设置页的提示条（is-info 等）与下方字段留出固定间距，避免贴在一起 */
.settings-page :deep(.hint-bar) {
  margin-bottom: var(--space-2);
}

/* 精简文案：设置页不再展示各分区的一长串介绍文字（只保留字段标签与操作提示） */
.settings-page :deep(.section-desc) {
  display: none;
}

/* 卡片内按钮统一由 flex/grid gap 控制间距，去掉 Element Plus 的相邻按钮 margin */
.settings-page :deep(.el-button + .el-button) {
  margin-left: 0;
}
</style>
