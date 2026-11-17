<!-- 云盘首页（W14 §8.2）
     正文只有三样：左上已登录网盘胶囊、右上账号管理入口、居中搜索框 + 热搜。
     账号管理从设置页迁移到这里的右上角设置图标（点击弹出账号管理抽屉）。
     KeepAlive include="CloudHomeView"：从搜索/管理返回时保留输入与滚动。 -->
<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import PageScroll from '@/components/PageScroll.vue'
import HintBar from '@/components/HintBar.vue'
import AccountDrawer from '@/components/cloud/AccountDrawer.vue'
import * as api from '@/api'
import type { PanAccountView } from '@/types'

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
  accountsOpen.value = true
}

function openPill(account: PanAccountView) {
  if (account.status === 'expired') {
    // 过期胶囊：跳账号管理并让用户重新登录
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
  // 解析失败「去登录」等外部入口：/cloud?accounts=1 直接打开账号管理抽屉
  if (String(route.query.accounts ?? '') === '1') accountsOpen.value = true
})

watch(
  () => route.query.accounts,
  (value) => {
    if (String(value ?? '') === '1') accountsOpen.value = true
  },
)

watch(accountsOpen, (open) => {
  // 关闭时刷新胶囊（抽屉里可能刚登录/退出）
  if (!open) void loadAccounts()
})
</script>

<template>
  <PageScroll>
    <div class="cloud-page">
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
              :size="15"
            />
            <span>{{ providerName(account.provider) }}</span>
            <span
              v-if="account.status === 'expired'"
              class="cloud-pill-badge"
            >!</span>
          </button>
        </div>

        <button
          class="cloud-account-btn"
          type="button"
          title="账号管理"
          aria-label="账号管理"
          @click="openAccountManage"
        >
          <SolarIcon
            name="settings"
            :size="18"
          />
        </button>
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

        <HintBar v-if="api.demoMode">
          浏览器演示模式：云盘搜索与账号登录只在桌面端（Tauri）可用。
        </HintBar>
      </div>

      <AccountDrawer v-model="accountsOpen" />
    </div>
  </PageScroll>
</template>

<style scoped>
.cloud-page {
  display: flex;
  flex-direction: column;
  min-height: 100%;
  padding: var(--space-5) var(--space-6);
  gap: var(--space-4);
}

.cloud-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  min-height: 32px;
}

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
  padding: 5px 12px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
  transition: border-color 0.15s ease, background-color 0.15s ease;
}

.cloud-pill:hover {
  border-color: var(--accent);
}

.cloud-pill.is-expired {
  color: var(--text-muted);
  border-style: dashed;
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

.cloud-account-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: 1px solid var(--border);
  border-radius: 50%;
  background: var(--surface);
  color: var(--text-muted);
  cursor: pointer;
  transition: color 0.15s ease, border-color 0.15s ease;
}

.cloud-account-btn:hover {
  color: var(--text);
  border-color: var(--accent);
}

.cloud-center {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-4);
  padding-bottom: 8vh;
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
</style>