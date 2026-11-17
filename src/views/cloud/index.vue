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
import DoubanCard from '@/views/vod/components/DoubanCard.vue'
import { proxyImage } from '@/utils/image'
import * as api from '@/api'
import type { DoubanHome, DoubanItem, PanAccountView } from '@/types'

defineOptions({ name: 'CloudHomeView' })

const router = useRouter()
const route = useRoute()

/** 网盘展示名与图标（与 W13 账号面板的 provider 顺序一致） */
const PROVIDER_META: Record<string, { name: string; icon: 'cloud-storage' }> = {
  p115: { name: '115', icon: 'cloud-storage' },
  quark: { name: '夸克', icon: 'cloud-storage' },
  xunlei: { name: '迅雷', icon: 'cloud-storage' },
  ali: { name: '阿里', icon: 'cloud-storage' },
  uc: { name: 'UC', icon: 'cloud-storage' },
  pikpak: { name: 'PikPak', icon: 'cloud-storage' },
}

const DEFAULT_HOT = ['三体', '流浪地球3', '沙丘2', '庆余年2', '4K合集']

const keyword = ref('')
const hot = ref<string[]>(DEFAULT_HOT)
const accounts = ref<PanAccountView[]>([])
const accountsOpen = ref(false)
/** 过期胶囊入口：打开弹窗后要聚焦的 provider（W14 §8.2） */
const accountsFocus = ref('')

/* ---------------- 豆瓣热榜（与点播页同源） ---------------- */

/** 自动轮播间隔（与点播页 HomeCarousel 一致） */
const INTERVAL_MS = 6000

const douban = ref<DoubanHome | null>(null)
const doubanLoading = ref(false)
const doubanIndex = ref(0)
const stripRef = ref<HTMLElement | null>(null)
let doubanTimer: number | undefined
const doubanPaused = ref(false)
const activated = ref(true)

const carouselItems = computed<DoubanItem[]>(() => douban.value?.carousel ?? [])
const activeDouban = computed<DoubanItem | undefined>(() => carouselItems.value[doubanIndex.value])
const hasMultiple = computed(() => carouselItems.value.length > 1)

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
    scrollActiveIntoView()
    schedule()
  }, INTERVAL_MS)
}

/** 让当前选中的卡片保持在可视区内（只滚卡片条，不带动整页） */
function scrollActiveIntoView() {
  const strip = stripRef.value
  if (!strip) return
  const child = strip.children[doubanIndex.value] as HTMLElement | undefined
  if (!child) return
  const left = child.offsetLeft - (strip.clientWidth - child.clientWidth) / 2
  strip.scrollTo({ left: Math.max(0, left), behavior: 'smooth' })
}

function goToDouban(target: number) {
  if (target === doubanIndex.value) return
  doubanIndex.value = target
  scrollActiveIntoView()
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
    schedule()
  },
)

/** 点击热榜卡片：直接以片名搜索云盘资源 */
function selectDouban(item: DoubanItem) {
  const title = item.title.trim()
  if (title) submitSearch(title)
}

/** 只渲染已登录且凭据有效的网盘；过期也显示（置灰）以便提示重新登录 */
const pills = computed(() =>
  accounts.value
    .filter((item) => item.status === 'connected' || item.status === 'expired')
    .sort((a, b) => {
      const order = Object.keys(PROVIDER_META)
      return order.indexOf(a.provider) - order.indexOf(b.provider)
    }),
)

function providerName(provider: string): string {
  return PROVIDER_META[provider]?.name ?? provider
}

async function loadAccounts() {
  if (api.demoMode) return
  try {
    accounts.value = await api.panAccountsList()
  } catch {
    // 首页降级为无胶囊，不阻塞搜索
    accounts.value = []
  }
}

async function loadHot() {
  if (api.demoMode) return
  try {
    const list = await api.cloudHotSearches()
    if (list.length) hot.value = list
  } catch {
    // 热搜失败保留默认词
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
  void router.push(`/cloud/search/${encodeURIComponent(kw)}`)
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter') submitSearch()
}

onMounted(() => {
  void loadAccounts()
  void loadHot()
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

onBeforeUnmount(clearTimer)

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
    void loadAccounts()
  }
})
</script>

<template>
  <PageScroll>
    <div class="cloud-page">
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

      <header class="cloud-top">
        <div class="cloud-pills">
          <button
            v-for="account in pills"
            :key="account.account_id"
            class="cloud-pill"
            :class="{ 'is-expired': account.status === 'expired' }"
            type="button"
            :title="account.status === 'expired' ? '凭据已过期，点击重新登录' : `打开 ${providerName(account.provider)} 云盘`"
            @click="openPill(account)"
          >
            <SolarIcon
              name="cloud-storage"
              :size="14"
            />
            <span>{{ providerName(account.provider) }}</span>
            <span
              v-if="account.status === 'expired'"
              class="cloud-pill-badge"
            >!</span>
          </button>
        </div>

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
      </header>

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

        <div class="cloud-hot">
          <span class="cloud-hot-label">热搜：</span>
          <button
            v-for="word in hot"
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
              自动轮播 · 点击卡片搜索资源
            </span>
          </div>
          <div
            ref="stripRef"
            class="douban-strip"
          >
            <div
              v-for="(item, index) in carouselItems"
              :key="item.id"
              class="douban-slide"
              :class="{ 'is-active': index === doubanIndex }"
            >
              <DoubanCard
                :item="item"
                :image-proxy="douban?.imageProxy"
                @select="selectDouban"
              />
              <button
                class="douban-dot"
                type="button"
                :aria-label="`选中《${item.title}》`"
                :aria-pressed="index === doubanIndex"
                @click.stop="goToDouban(index)"
              />
            </div>
          </div>
        </section>

        <HintBar v-if="api.demoMode">
          浏览器演示模式：云盘搜索与账号登录只在桌面端（Tauri）可用。
        </HintBar>
      </div>

      <AccountDrawer
        v-model="accountsOpen"
        :focus-provider="accountsFocus"
      />
    </div>
  </PageScroll>
</template>

<style scoped>
.cloud-page {
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 100%;
  padding: var(--space-5) var(--space-6);
  gap: var(--space-4);
  overflow: hidden;
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
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  min-height: 32px;
}

/* 左上胶囊：与点播页分类 chip 同款（无边框、surface-2 底），但页面不加渐变背景 */
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

.cloud-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: 0 0 auto;
  padding: 7px 14px;
  border: 1px solid transparent;
  border-radius: var(--radius-pill);
  background: var(--surface-2);
  color: var(--text-regular);
  font-size: 13px;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.cloud-pill:hover {
  background: var(--surface-3);
  color: var(--text);
}

.cloud-pill.is-expired {
  opacity: 0.68;
}

.cloud-pill-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--warning);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
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
  padding-top: 5vh;
  padding-bottom: var(--space-5);
}

.cloud-hero {
  text-align: center;
}

.cloud-title {
  margin: 0 0 6px;
  font-size: 28px;
  font-weight: 700;
  color: var(--text);
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
  padding: 6px 6px 6px 14px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--surface);
  transition: border-color 0.15s ease;
}

.cloud-search:focus-within {
  border-color: var(--accent);
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
  padding: 6px 18px;
  border: 0;
  border-radius: 8px;
  background: var(--accent);
  color: #fff;
  font-size: 13px;
  cursor: pointer;
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
  width: min(1180px, 100%);
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

.douban-strip {
  display: flex;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-3) var(--space-2) var(--space-4);
  overflow-x: auto;
  scrollbar-width: thin;
  scroll-snap-type: x proximity;
}

.douban-slide {
  position: relative;
  flex: 0 0 136px;
  width: 136px;
  opacity: 0.72;
  scroll-snap-align: center;
  transition:
    transform 0.35s ease,
    opacity 0.35s ease;
}

.douban-slide.is-active {
  z-index: 1;
  opacity: 1;
  transform: translateY(-6px) scale(1.04);
}

.douban-dot {
  position: absolute;
  bottom: -2px;
  left: 50%;
  width: 6px;
  height: 6px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: var(--border);
  opacity: 0;
  transform: translateX(-50%);
  cursor: pointer;
}

.douban-slide.is-active .douban-dot {
  background: var(--accent);
  opacity: 1;
}
</style>