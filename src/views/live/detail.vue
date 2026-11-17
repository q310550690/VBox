<!-- 直播频道详情（/live/detail/:group/:channel）
     从频道列表点进来，展示频道信息、多源地址、请求头与节目单。
     直播播放器尚未接入，这里只展示真实解析结果，不伪造播放。 -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SolarIcon from '@/components/SolarIcon.vue'
import EmptyState from '@/components/EmptyState.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import EpgStrip from './components/EpgStrip.vue'
import * as api from '@/api'
import { useConfigStore } from '@/store/config'
import type { EpgProgram, LiveChannel, LiveGroup, LiveSource } from '@/types'

defineOptions({ name: 'LiveDetailView' })

/** 支持的直播源类型：0=文本订阅（txt/m3u）、3=js spider */
const SUPPORTED_LIVE_TYPES = [0, 3]

const route = useRoute()
const router = useRouter()
const config = useConfigStore()

const groupKey = computed(() => String(route.params.group ?? ''))
const channelName = computed(() => String(route.params.channel ?? ''))

const loading = ref(false)
const error = ref('')
const sources = ref<LiveSource[]>([])
const sourceIndex = ref(-1)
const group = ref<LiveGroup | null>(null)
const channel = ref<LiveChannel | null>(null)
const activeUrlIndex = ref(0)
const programs = ref<EpgProgram[]>([])
const epgLoading = ref(false)
const epgError = ref('')
/** 密码分组：记录已解锁的 group key，换组后需要重新解锁 */
const unlockedGroup = ref('')
const passwordInput = ref('')
const unlocking = ref(false)

let epgSeq = 0

const currentSource = computed<LiveSource | null>(() =>
  sourceIndex.value >= 0 ? sources.value[sourceIndex.value] ?? null : null,
)
const locked = computed(
  () => Boolean(group.value?.password) && unlockedGroup.value !== group.value?.key,
)
const urlOptions = computed(() => channel.value?.urls ?? [])
const currentUrl = computed(() => urlOptions.value[activeUrlIndex.value] ?? null)
const headers = computed(() => {
  const map = channel.value?.header ?? {}
  return Object.entries(map).map(([key, value]) => `${key}: ${value}`)
})
const initial = computed(() => channel.value?.name.trim().charAt(0) || '频')

function today(): string {
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

/** EPG 地址：优先 lives 组级 epg → m3u 组级 epg → 频道 epg（仅接受 http(s) URL） */
function resolveEpgUrl(target: LiveChannel): string {
  const sourceEpg = currentSource.value?.epg
  if (sourceEpg && /^https?:\/\//i.test(sourceEpg)) return sourceEpg
  const groupEpg = group.value?.epg
  if (groupEpg && /^https?:\/\//i.test(groupEpg)) return groupEpg
  if (target.epg && /^https?:\/\//i.test(target.epg)) return target.epg
  return ''
}

async function loadEpg() {
  const target = channel.value
  if (!target) return
  const epgUrl = resolveEpgUrl(target)
  programs.value = []
  epgError.value = ''
  if (!epgUrl) return
  const seq = ++epgSeq
  epgLoading.value = true
  try {
    const list = await api.liveEpg({
      epgUrl,
      channel: target.name,
      date: today(),
    })
    if (seq !== epgSeq) return
    // XMLTV 往往是整站节目单：优先过滤当前频道，无匹配时退回完整列表
    const wanted = [target.epg, target.name]
      .filter((value): value is string => Boolean(value))
      .map((value) => value.toLowerCase())
    const matched = list.filter(
      (program) => program.channel && wanted.includes(program.channel.toLowerCase()),
    )
    programs.value = matched.length ? matched : list
  } catch (e) {
    if (seq !== epgSeq) return
    epgError.value = api.formatError(e)
  } finally {
    if (seq === epgSeq) epgLoading.value = false
  }
}

/** 在已加载的分组里定位当前频道 */
function pickChannel() {
  const targetGroup = group.value
  if (!targetGroup) return
  const found = (targetGroup.channels ?? []).find((item) => item.name === channelName.value)
  if (!found) {
    channel.value = null
    error.value = `频道「${channelName.value || '未命名'}」不存在或已从当前分组移除`
    return
  }
  channel.value = found
  activeUrlIndex.value = 0
  void loadEpg()
}

async function load() {
  if (!config.initialized) return
  loading.value = true
  error.value = ''
  channel.value = null
  group.value = null
  programs.value = []
  try {
    sources.value = await api.liveSources()
    const index = sources.value.findIndex((source) =>
      SUPPORTED_LIVE_TYPES.includes(source.type ?? -1),
    )
    if (index < 0) {
      error.value = '当前订阅没有可用的直播源'
      return
    }
    sourceIndex.value = index
    const groups = await api.liveGroups(index)
    const target = groups.find((item) => item.key === groupKey.value)
    if (!target) {
      error.value = `直播分组「${groupKey.value || '未命名'}」不存在`
      return
    }
    group.value = target
    if (target.password && unlockedGroup.value !== target.key) {
      // 密码分组先解锁，不提前展示频道；同一次会话内已解锁的组直接进入
      return
    }
    pickChannel()
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

function unlock() {
  const target = group.value
  if (!target) return
  unlocking.value = true
  try {
    if (passwordInput.value !== target.password) {
      error.value = '分组密码错误'
      return
    }
    unlockedGroup.value = target.key
    error.value = ''
    passwordInput.value = ''
    pickChannel()
  } finally {
    unlocking.value = false
  }
}

function retry() {
  unlockedGroup.value = ''
  passwordInput.value = ''
  void load()
}

function goBack() {
  void router.push('/live')
}

watch(
  () => config.initialized,
  (ready) => {
    if (ready) void load()
  },
  { immediate: true },
)

// 频道切换时重新加载节目单（同组件内路由参数变化）
watch([groupKey, channelName], () => {
  if (config.initialized) void load()
})
</script>

<template>
  <PageScroll>
    <div class="page live-detail-page">
    <header class="page-header">
      <div class="heading">
        <el-button circle aria-label="返回直播" @click="goBack">
          <template #icon><SolarIcon name="arrow-left" /></template>
        </el-button>
        <h1 class="page-title ellipsis-1">
          {{ channel?.name || channelName || '直播详情' }}
        </h1>
        <span v-if="group" class="muted">{{ group.name }}</span>
      </div>
    </header>

    <SkeletonGrid v-if="loading" variant="card" :count="3" />

    <ErrorState
      v-else-if="error && !locked"
      :message="error"
      title="直播频道加载失败"
      retry-text="重试"
      @retry="retry"
    />

    <section v-else-if="locked" class="lock-panel" aria-label="分组密码">
      <SolarIcon class="lock-icon" name="lock" />
      <p class="lock-title">分组「{{ group?.name }}」需要密码</p>
      <p class="lock-desc">输入分组密码后查看频道；密码仅保存在本次会话。</p>
      <p v-if="error" class="lock-error">{{ error }}</p>
      <div class="lock-form">
        <el-input
          v-model="passwordInput"
          type="password"
          show-password
          placeholder="分组密码"
          @keyup.enter="unlock"
        />
        <el-button type="primary" :loading="unlocking" @click="unlock">解锁</el-button>
      </div>
    </section>

    <template v-else-if="channel">
      <section class="channel-panel">
        <div class="channel-head">
          <span class="channel-logo" aria-hidden="true">
            <img
              v-if="channel.logo"
              class="logo-img"
              :src="channel.logo"
              :alt="channel.name"
              loading="lazy"
            />
            <template v-else>{{ initial }}</template>
          </span>
          <div class="channel-meta">
            <h2 class="channel-name">{{ channel.name }}</h2>
            <p class="channel-group">
              {{ group?.name }}<span v-if="currentSource?.name"> · {{ currentSource.name }}</span>
            </p>
          </div>
        </div>

        <EmptyState
          v-if="!urlOptions.length"
          title="该频道没有可用播放地址"
          desc="直播源解析到了频道，但没有返回可播放的 URL。"
        />
        <div v-else class="url-list">
          <button
            v-for="(item, index) in urlOptions"
            :key="`${item.url}-${index}`"
            type="button"
            class="url-item"
            :class="{ active: index === activeUrlIndex }"
            @click="activeUrlIndex = index"
          >
            <span class="url-name">{{ item.name || `源${index + 1}` }}</span>
            <span class="url-value mono ellipsis-1" :title="item.url">{{ item.url }}</span>
          </button>
        </div>

        <p v-if="currentUrl" class="current-url mono" :title="currentUrl.url">
          {{ currentUrl.url }}
        </p>
        <p v-if="headers.length" class="meta-line">请求头：{{ headers.join(' · ') }}</p>
        <p v-if="channel.referer" class="meta-line">Referer：{{ channel.referer }}</p>
        <p class="hint">播放器待接入：当前仅展示直播地址与请求头，不进行播放。</p>
      </section>

      <EpgStrip
        :channel="channel"
        :programs="programs"
        :loading="epgLoading"
        :error="epgError"
        @retry="loadEpg"
      />
    </template>
    </div>
  </PageScroll>
</template>

<style scoped>
.page-header .heading {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.channel-panel {
  margin-top: var(--space-3);
  padding: var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  background: var(--surface);
}

.channel-head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.channel-logo {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 48px;
  width: 48px;
  height: 48px;
  border-radius: var(--radius-md);
  background: var(--surface-2);
  color: var(--text-muted);
  font-size: 18px;
  font-weight: 600;
  overflow: hidden;
}

.logo-img {
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: var(--surface);
}

.channel-meta {
  min-width: 0;
}

.channel-name {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  color: var(--text);
}

.channel-group {
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

.url-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin-top: var(--space-4);
}

.url-item {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text-regular);
  text-align: left;
  cursor: pointer;
  transition: var(--transition-color);
}

.url-item:hover {
  border-color: var(--accent);
}

.url-item.active {
  border-color: var(--accent);
  background: var(--accent-weak);
  color: var(--accent);
}

.url-name {
  flex: 0 0 auto;
  font-size: 12px;
  font-weight: 600;
}

.url-value {
  min-width: 0;
  font-size: 12px;
}

.current-url {
  margin: var(--space-3) 0 0;
  overflow: hidden;
  font-size: 12px;
  color: var(--text-regular);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.meta-line {
  margin: var(--space-1) 0 0;
  overflow: hidden;
  font-size: 11px;
  color: var(--text-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hint {
  margin: var(--space-2) 0 0;
  font-size: 11px;
  color: var(--text-faint);
}

.lock-panel {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-5) var(--space-4);
  border: 1px dashed var(--border);
  border-radius: var(--radius-lg);
  background: var(--surface-2);
  text-align: center;
}

.lock-icon {
  font-size: 28px;
  color: var(--text-muted);
}

.lock-title {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.lock-desc {
  margin: 0;
  font-size: 12px;
  color: var(--text-faint);
}

.lock-error {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
}

.lock-form {
  display: flex;
  gap: var(--space-2);
  width: min(360px, 100%);
}
</style>
