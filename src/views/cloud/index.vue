<!-- 云盘首页（W14 §8.2）
     正文：左上已登录网盘胶囊（与点播页 chip 同款、无渐变底）、右上账号管理入口、
     居中搜索框 + 热搜 + 豆瓣热榜（通用海报卡依次排列、自动轮播选中，整页模糊封面背景）。
     账号管理从设置页迁移到这里的右上角按钮（点击弹出账号管理弹窗）。
     KeepAlive include="CloudHomeView"：从搜索/管理返回时保留输入与滚动。 -->
<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import PageScroll from '@/components/PageScroll.vue'
import HintBar from '@/components/HintBar.vue'
import AccountDrawer from '@/components/cloud/AccountDrawer.vue'
import StatusTabs from '@/components/StatusTabs.vue'
import PageHead from '@/components/PageHead.vue'
import DoubanCard from '@/views/vod/components/DoubanCard.vue'
import { proxyImage } from '@/utils/image'
import * as api from '@/api'
import { useCloudAccountsStore } from '@/store/cloudAccounts'
import type { DoubanHome, DoubanItem, PanAccountView } from '@/types'

defineOptions({ name: 'CloudHomeView' })

const router = useRouter()
const route = useRoute()

const HISTORY_KEY = 'vbox.cloud.searchHistory'
const HISTORY_MAX = 8

const keyword = ref('')
const searchHistory = ref<string[]>([])
const cloudAccounts = useCloudAccountsStore()
const accountsOpen = ref(false)
/** 过期胶囊入口：打开弹窗后要聚焦的 provider（W14 §8.2） */
const accountsFocus = ref('')

/* ---------------- 豆瓣热榜（与点播页同源） ---------------- */

/** 自动轮播间隔（与点播页 HomeCarousel 一致） */
const INTERVAL_MS = 6000

/** 一页显示的卡片数；两端箭头整页翻动 */
const PAGE_SIZE = 6

const douban = ref<DoubanHome | null>(null)
const doubanLoading = ref(false)
const doubanIndex = ref(0)
const doubanPage = ref(0)
let doubanTimer: number | undefined
const doubanPaused = ref(false)
const activated = ref(true)

const carouselItems = computed<DoubanItem[]>(() => douban.value?.carousel ?? [])
const activeDouban = computed<DoubanItem | undefined>(() => carouselItems.value[doubanIndex.value])
const hasMultiple = computed(() => carouselItems.value.length > 1)

/** 按 6 张一页切块；最后一页可以不满 */
const doubanPageItems = computed<DoubanItem[][]>(() => {
  const pages: DoubanItem[][] = []
  for (let i = 0; i < carouselItems.value.length; i += PAGE_SIZE) {
    pages.push(carouselItems.value.slice(i, i + PAGE_SIZE))
  }
  return pages
})
const doubanPages = computed(() => Math.max(1, doubanPageItems.value.length))
const canDoubanPrev = computed(() => doubanPage.value > 0)
const canDoubanNext = computed(() => doubanPage.value < doubanPages.value - 1)

/** 当前选中卡片的封面（经本机图片代理，豆瓣封面需 Referer） */
const activeCover = computed(() => {
  const item = activeDouban.value
  if (!item) return ''
  return proxyImage(item.cover, douban.value?.imageProxy)
})

const backdrop = computed(() => {
  if (!activeCover.value) return ''
  return `url("${activeCover.value.replace(/"/g, '%22')}")`
})

function clearTimer() {
  if (doubanTimer !== undefined) {
    window.clearTimeout(doubanTimer)
    doubanTimer = undefined
  }
}

function schedule() {
  clearTimer()
  if (!hasMultiple.value || doubanPaused.value || !activated.value) return
  doubanTimer = window.setTimeout(() => {
    doubanIndex.value = (doubanIndex.value + 1) % carouselItems.value.length
    syncDoubanPage()
    schedule()
  }, INTERVAL_MS)
}

/** 让页码跟随当前选中的卡片（自动轮播跨页时自动翻页） */
function syncDoubanPage() {
  doubanPage.value = Math.min(doubanPages.value - 1, Math.floor(doubanIndex.value / PAGE_SIZE))
}

/** 箭头翻页：整页翻动，选中落到该页第一张 */
function goDoubanPage(direction: -1 | 1) {
  const next = Math.max(0, Math.min(doubanPages.value - 1, doubanPage.value + direction))
  if (next === doubanPage.value) return
  doubanPage.value = next
  const first = next * PAGE_SIZE
  if (first < carouselItems.value.length) doubanIndex.value = first
  schedule()
}

function pause() {
  doubanPaused.value = true
  clearTimer()
}

function resume() {
  doubanPaused.value = false
  schedule()
}

async function loadDouban() {
  // 浏览器演示模式也展示 mock 豆瓣数据，便于预览首页视觉
  doubanLoading.value = true
  try {
    // 优先展示后端缓存，避免首页白屏；随后再拉一次（后端 10 分钟缓存）
    try {
      const cached = await api.doubanHomeCached()
      if (cached) douban.value = cached
    } catch {
      // 缓存不可用忽略
    }
    douban.value = await api.doubanHome(false)
    doubanIndex.value = Math.min(doubanIndex.value, Math.max(0, carouselItems.value.length - 1))
    syncDoubanPage()
    schedule()
  } catch {
    // 豆瓣不可用不阻塞云盘搜索
  } finally {
    doubanLoading.value = false
  }
}

watch(
  () => carouselItems.value.length,
  (total) => {
    if (total === 0) {
      doubanIndex.value = 0
      clearTimer()
      return
    }
    if (doubanIndex.value >= total) doubanIndex.value = 0
    syncDoubanPage()
    schedule()
  },
)

/** 点击热榜卡片：直接以片名搜索云盘资源 */
function selectDouban(item: DoubanItem) {
  const title = item.title.trim()
  if (title) submitSearch(title)
}

/** 只渲染已登录且凭据有效的网盘；过期也显示（置灰）以便提示重新登录 */
const pills = computed(() => cloudAccounts.pills)
/** 左上网盘 tab：与下载/设置页同款 StatusTabs；过期凭据置灰 + 「!」角标 */
const pillTabs = computed(() =>
  pills.value.map((account) => ({
    value: account.account_id,
    label: providerName(account.provider),
    muted: account.status === 'expired',
    badge: account.status === 'expired' ? '!' : undefined,
    title:
      account.status === 'expired'
        ? '凭据已过期，点击重新登录'
        : `打开 ${providerName(account.provider)} 云盘`,
  })),
)

function onPillTab(accountId: string) {
  const account = pills.value.find((a) => a.account_id === accountId)
  if (account) openPill(account)
}
const providerName = (provider: string): string => cloudAccounts.providerName(provider)

async function loadAccounts() {
  // 账号状态统一由 store 管理（W14 §7.3）；失败时保持无胶囊，不阻塞搜索
  await cloudAccounts.load()
}

function loadHistory() {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    const list = raw ? (JSON.parse(raw) as unknown) : []
    if (Array.isArray(list)) searchHistory.value = list.filter((w) => typeof w === 'string').slice(0, HISTORY_MAX)
  } catch {
    searchHistory.value = []
  }
}

function pushHistory(word: string) {
  const next = [word, ...searchHistory.value.filter((w) => w !== word)].slice(0, HISTORY_MAX)
  searchHistory.value = next
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next))
  } catch {
    // 隐私模式下写不进去就算了
  }
}

function openAccountManage() {
  accountsFocus.value = ''
  accountsOpen.value = true
}

function openPill(account: PanAccountView) {
  if (account.status === 'expired') {
    // 过期胶囊：打开账号管理弹窗并聚焦该网盘的「重新登录」（W14 §8.2）
    accountsFocus.value = account.provider
    accountsOpen.value = true
    return
  }
  void router.push(`/cloud/manage/${encodeURIComponent(account.account_id)}`)
}

function submitSearch(word?: string) {
  const kw = (word ?? keyword.value).trim()
  if (!kw) return
  keyword.value = kw
  pushHistory(kw)
  void router.push(`/cloud/search/${encodeURIComponent(kw)}`)
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter') submitSearch()
}

onMounted(() => {
  void loadAccounts()
  loadHistory()
  void loadDouban()
  // 解析失败「去登录」等外部入口：/cloud?accounts=1 直接打开账号管理弹窗
  if (String(route.query.accounts ?? '') === '1') accountsOpen.value = true
})

onActivated(() => {
  activated.value = true
  schedule()
})

onDeactivated(() => {
  activated.value = false
  clearTimer()
})

onBeforeUnmount(() => {
  clearTimer()
})

watch(
  () => route.query.accounts,
  (value) => {
    if (String(value ?? '') === '1') accountsOpen.value = true
  },
)

watch(accountsOpen, (open) => {
  // 关闭时刷新胶囊（弹窗里可能刚登录/退出），并清空聚焦状态
  if (!open) {
    accountsFocus.value = ''
    void cloudAccounts.refresh()
  }
})
</script>

<template>
  <PageScroll fill>
    <div class="page cloud-page">
      <!-- 整页模糊封面背景：跟随自动轮播选中的豆瓣条目 -->
      <div
        class="cloud-bg"
        aria-hidden="true"
      >
        <Transition name="cloud-bg-fade">
          <div
            v-if="backdrop"
            :key="activeDouban?.id ?? ''"
            class="cloud-bg-img"
            :style="{ backgroundImage: backdrop }"
          />
        </Transition>
      </div>

      <PageHead class="cloud-top">
        <StatusTabs
          v-if="pillTabs.length"
          :items="pillTabs"
          model-value=""
          aria-label="已登录网盘"
          @change="onPillTab"
          />
        <template #tools>
        <div class="page-toolbar">
          <div class="site-btn">
          <el-tooltip
            content="账号管理"
            placement="bottom"
          >
            <el-button
              circle
              aria-label="账号管理"
              @click="openAccountManage"
            >
              <template #icon>
                <SolarIcon
                  name="settings"
                  size="1em"
                />
              </template>
            </el-button>
          </el-tooltip>
          </div>
        </div>
        </template>
      </PageHead>

      <div class="cloud-center">
        <div class="cloud-hero">
          <h1 class="cloud-title">
            云盘聚合搜索
          </h1>
          <p class="cloud-sub">
            一次搜索，聚合夸克 / 阿里 / 115 / 迅雷 / PikPak 等网盘分享
          </p>
        </div>

        <div class="cloud-search">
          <SolarIcon
            class="cloud-search-icon"
            name="magnifier"
            :size="18"
          />
          <input
            v-model="keyword"
            class="cloud-input"
            type="text"
            placeholder="搜索全网云盘资源…"
            @keydown="onKeydown"
          >
          <button
            class="cloud-search-btn"
            type="button"
            @click="submitSearch()"
          >
            搜索
          </button>
        </div>

        <div
          v-if="searchHistory.length"
          class="cloud-hot"
        >
          <span class="cloud-hot-label">历史记录：</span>
          <button
            v-for="word in searchHistory"
            :key="word"
            class="cloud-hot-item"
            type="button"
            @click="submitSearch(word)"
          >
            {{ word }}
          </button>
        </div>

        <!-- 豆瓣热榜：通用海报卡依次排列，自动轮播选中一张 -->
        <section
          v-if="carouselItems.length"
          class="douban-block"
          aria-label="豆瓣热榜"
          @mouseenter="pause"
          @mouseleave="resume"
          @focusin="pause"
          @focusout="resume"
        >
          <div class="douban-head">
            <span class="douban-kicker">
              豆瓣热榜
            </span>
            <span class="douban-hint">
              自动轮播 · 左右翻页 · 点击卡片搜索资源
            </span>
          </div>
          <div class="douban-carousel">
            <div class="douban-viewport">
              <div
                class="douban-pages"
                :style="{ transform: `translateX(calc(${-doubanPage * 100}% - ${doubanPage * 32}px))` }"
              >
                <div
                  v-for="(page, pageIndex) in doubanPageItems"
                  :key="pageIndex"
                  class="douban-page"
                >
                  <div
                    v-for="(item, index) in page"
                    :key="item.id"
                    class="douban-slide"
                    :class="{ 'is-active': pageIndex * PAGE_SIZE + index === doubanIndex }"
                  >
                    <DoubanCard
                      :item="item"
                      :image-proxy="douban?.imageProxy"
                      @select="selectDouban"
                    />
                  </div>
                </div>
              </div>
            </div>
            <el-tooltip
              content="上一页"
              placement="bottom"
              :show-after="300"
              :disabled="!canDoubanPrev"
            >
              <el-button
                v-show="canDoubanPrev"
                class="douban-arrow is-left"
                circle
                aria-label="上一页"
                @click="goDoubanPage(-1)"
              >
                <SolarIcon
                  name="alt-arrow-left"
                  :size="16"
                />
              </el-button>
            </el-tooltip>
            <el-tooltip
              content="下一页"
              placement="bottom"
              :show-after="300"
              :disabled="!canDoubanNext"
            >
              <el-button
                v-show="canDoubanNext"
                class="douban-arrow is-right"
                circle
                aria-label="下一页"
                @click="goDoubanPage(1)"
              >
                <SolarIcon
                  name="alt-arrow-right"
                  :size="16"
                />
              </el-button>
            </el-tooltip>
          </div>
        </section>

      </div>

      <HintBar v-if="api.demoMode">
        浏览器演示模式：云盘搜索与账号登录只在桌面端（Tauri）可用。
      </HintBar>

      <AccountDrawer
        v-model="accountsOpen"
        :focus-provider="accountsFocus"
      />
    </div>
  </PageScroll>
</template>

<style scoped>
/* 页面容器沿用全局 .page 的内边距与块间距；自身撑满视口高度，模糊背景才有整页可铺 */
.cloud-page {
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 100%;
}

/* —— 整页模糊背景（与点播页 Hero 背景同款：radial 渐隐 + blur） —— */
.cloud-bg {
  position: absolute;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  pointer-events: none;
  mask-image: radial-gradient(ellipse 60% 65% at 50% 42%, #000 28%, transparent 92%);
  -webkit-mask-image: radial-gradient(ellipse 60% 65% at 50% 42%, #000 28%, transparent 92%);
}

.cloud-bg-img {
  position: absolute;
  inset: 0;
  background-position: center;
  background-size: cover;
  opacity: 0.32;
  filter: blur(46px) saturate(1.25);
  transform: scale(1.06);
}

.cloud-bg-fade-enter-active,
.cloud-bg-fade-leave-active {
  transition: opacity 0.8s ease;
}

.cloud-bg-fade-enter-from,
.cloud-bg-fade-leave-to {
  opacity: 0;
}

.cloud-top {
  position: relative;
  z-index: 1;
  min-height: 32px;
}

/* 左上网盘 tab：与下载/设置页同款 StatusTabs（页面不加渐变背景） */
.cloud-pills {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  overflow-x: auto;
  scrollbar-width: none;
}

.cloud-pills::-webkit-scrollbar {
  display: none;
}

.cloud-pills :deep(.status-tabs) {
  flex-wrap: nowrap;
}

/* 右上角圆形按钮：与点播页搜索按钮（.site-btn > el-button circle）一致 */
.site-btn {
  display: flex;
  flex-shrink: 0;
  align-items: center;
}

.cloud-center {
  position: relative;
  z-index: 1;
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  gap: var(--space-4);
  padding-top: var(--space-2);
  padding-bottom: var(--space-2);
}

.cloud-hero {
  text-align: center;
}

.cloud-title {
  margin: 0 0 6px;
  font-size: var(--fs-display);
  font-weight: 600;
  color: var(--text);
  letter-spacing: -0.2px;
}

.cloud-sub {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
}

.cloud-search {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: min(680px, 82vw);
  padding: 4px 4px 4px 12px;
  border: 0;
  box-shadow: var(--shadow-1);
  border-radius: var(--radius-md);
  background: var(--surface-card);
  transition: box-shadow .16s var(--ease);
}

/* 容器本身无边框，焦点只能画在容器上：与外层卡片影叠加一条 2px 主色光环 */
.cloud-search:focus-within {
  box-shadow:
    var(--shadow-1),
    0 0 0 2px color-mix(in srgb, var(--el-color-primary) 55%, var(--surface-card));
}

.cloud-search-icon {
  color: var(--text-muted);
  flex: 0 0 auto;
}

.cloud-input {
  flex: 1;
  min-width: 0;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  line-height: 28px;
}

.cloud-input::placeholder {
  color: var(--text-muted);
}

.cloud-search-btn {
  flex: 0 0 auto;
  height: 32px;
  padding: 0 14px;
  border: 0;
  border-radius: var(--radius-md);
  background: var(--accent-solid);
  color: var(--accent-on);
  font-size: 13px;
  font-weight: 500;
  line-height: 1;
  cursor: pointer;
  transition: background-color .18s var(--ease), box-shadow .18s var(--ease);
}

.cloud-search-btn:hover {
  background: var(--accent-solid-hover);
}

.cloud-search-btn:active {
  background: var(--accent-solid-active);
}

.cloud-search-btn:focus-visible {
  outline: none;
  box-shadow: var(--ring);
}

.cloud-hot {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: 6px 10px;
  max-width: min(680px, 82vw);
  font-size: 12px;
  color: var(--text-muted);
}

.cloud-hot-label {
  flex: 0 0 auto;
}

.cloud-hot-item {
  padding: 2px 8px;
  border: 0;
  border-radius: 999px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  cursor: pointer;
}

.cloud-hot-item:hover {
  color: var(--accent);
  background: var(--surface-2);
}

/* —— 豆瓣热榜：卡片依次排列 + 自动轮播选中 —— */
.douban-block {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  width: 100%;
  margin-top: var(--space-2);
}

.douban-head {
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: var(--space-2);
}

.douban-kicker {
  color: var(--accent);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.04em;
}

.douban-hint {
  color: var(--text-muted);
  font-size: 12px;
}

.douban-carousel {
  position: relative;
}

/* 翻页视口：不滚动，靠页码整页平移；内边距给卡片阴影留绘制空间，
   负外边距把多出来的盒子拉回去，卡片仍与页面内容左缘对齐 */
.douban-viewport {
  position: relative;
  overflow: hidden;
  /* 阴影绘制区：内边距 + 等量负外边距，裁切盒外扩但不挤占卡片宽度 */
  padding: 6px 16px 32px;
  margin: -6px -16px -32px;
}

.douban-pages {
  display: flex;
  width: 100%;
  /* 与视口外扩的阴影区等宽：翻页空隙落在裁切盒外，相邻页卡片不会漏边 */
  gap: 32px;
  transition: transform 0.5s var(--ease);
  will-change: transform;
}

.douban-page {
  display: flex;
  flex: 0 0 100%;
  min-width: 0;
  gap: var(--space-3);
}

/* 两端翻页箭头：静止隐藏，悬停整块时浮出（与 StatusTabs 同款圆钮） */
.douban-arrow {
  position: absolute;
  top: 50%;
  z-index: 2;
  opacity: 0;
  transform: translateY(-50%);
  pointer-events: none;
  box-shadow: var(--shadow-1);
  transition:
    opacity 0.16s var(--ease),
    transform 0.16s var(--ease),
    background-color 0.16s var(--ease);
}

.douban-arrow.is-left {
  left: 2px;
}

.douban-arrow.is-right {
  right: 2px;
}

.douban-block:hover .douban-arrow,
.douban-arrow:focus-visible {
  opacity: 1;
  pointer-events: auto;
}

/* 去掉圆点后不再用"压暗非当前项"表达焦点：整排同亮度，靠 hover 抬起 */
.douban-slide {
  position: relative;
  flex: 0 0 calc((100% - 5 * var(--space-3)) / 6);
  min-width: 0;
  transition: transform 0.28s var(--ease);
}

.douban-slide:hover {
  transform: translateY(-3px);
}

.douban-slide.is-active {
  z-index: 1;
}

/* 当前轮播到的海报：沿用首页 hover 的抬升/大阴影，再补一圈主色描边做选中态 */
.douban-slide.is-active :deep(.poster-card) {
  box-shadow:
    var(--shadow-2),
    0 0 0 2px var(--accent);
  transform: translateY(-2px);
}

</style>
