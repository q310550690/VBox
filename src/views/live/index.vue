<!-- 直播页（frontend-design.md §6.5）：真实 lives 订阅 → live_groups → 分组/密码锁/多源频道/EPG
     播放器仍未接入：点击频道只展示解析到的直播地址与请求头，不假装播放。 -->
<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import * as api from '@/api'
import ChannelCard from './components/ChannelCard.vue'
import EmptyState from '@/components/EmptyState.vue'
import EpgStrip from './components/EpgStrip.vue'
import ErrorState from '@/components/ErrorState.vue'
import PageScroll from '@/components/PageScroll.vue'
import LiveGroupNav from './components/LiveGroupNav.vue'
import SkeletonGrid from '@/components/SkeletonGrid.vue'
import { useConfigStore } from '@/store/config'
import { useUiStore } from '@/store/ui'
import type { EpgProgram, LiveChannel, LiveGroup, LiveSource } from '@/types'

defineOptions({ name: 'LiveView' })

const router = useRouter()
const ui = useUiStore()
const config = useConfigStore()

/** 支持的直播源类型：0=文本订阅（txt/m3u）、3=js spider；jar/py 后端会返回明确错误 */
const SUPPORTED_LIVE_TYPES = [0, 3]

const sources = ref<LiveSource[]>([])
const sourceIndex = ref(-1)
const groups = ref<LiveGroup[]>([])
const activeGroupKey = ref('')
const activeChannel = ref<LiveChannel | null>(null)
const activeUrlIndex = ref(0)
const programs = ref<EpgProgram[]>([])
/** 已解锁的密码分组：group.key → 用户输入的密码 */
const unlocked = ref<Record<string, string>>({})

const loadingSources = ref(false)
const loadingGroups = ref(false)
const loadingEpg = ref(false)
const sourceError = ref('')
const groupError = ref('')
const epgError = ref('')

const demoMode = api.demoMode
/** Tauri 桌面端：等 config.init() 完成后再判断是否有直播源，避免误判空态 */
const waitingConfig = computed(() => api.isTauri && !config.initialized)

const currentSource = computed<LiveSource | null>(() =>
  sourceIndex.value >= 0 ? sources.value[sourceIndex.value] ?? null : null,
)
const activeGroup = computed<LiveGroup | null>(
  () => groups.value.find((group) => group.key === activeGroupKey.value) ?? null,
)
const activeGroupLocked = computed(() => {
  const group = activeGroup.value
  if (!group?.password) return false
  return unlocked.value[group.key] !== group.password
})
const channels = computed<LiveChannel[]>(() =>
  activeGroupLocked.value ? [] : (activeGroup.value?.channels ?? []),
)
/** Tauri 模式下当前订阅没有任何 type0/3 的直播源 */
const noLiveSource = computed(
  () =>
    api.isTauri &&
    config.initialized &&
    !loadingSources.value &&
    !sources.value.some((source) => SUPPORTED_LIVE_TYPES.includes(source.type ?? -1)),
)

const urlOptions = computed(() => activeChannel.value?.urls ?? [])
const currentPlayUrl = computed(() => urlOptions.value[activeUrlIndex.value]?.url ?? '')
const currentSourceName = computed(
  () => urlOptions.value[activeUrlIndex.value]?.name || `源${activeUrlIndex.value + 1}`,
)
const currentHeaders = computed(() =>
  Object.entries(activeChannel.value?.header ?? {}).map(([key, value]) => `${key}: ${value}`),
)
const currentReferer = computed(() => activeChannel.value?.referer || '')

/** 请求序号：快速切换分组/频道时丢弃过期响应 */
let groupSeq = 0
let epgSeq = 0
/** 加载直播源期间又切换了启用线路：等当前请求结束后再刷一次 */
let reloadPending = false

/** 当天日期（live_epg 的 {date} 模板用） */
function today(): string {
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

/* ---------------- 数据加载 ---------------- */

async function loadSources() {
  if (waitingConfig.value) return
  if (loadingSources.value) {
    reloadPending = true
    return
  }
  loadingSources.value = true
  sourceError.value = ''
  groups.value = []
  activeGroupKey.value = ''
  activeChannel.value = null
  programs.value = []
  epgError.value = ''
  unlocked.value = {}
  try {
    sources.value = await api.liveSources()
    const index = sources.value.findIndex((source) =>
      SUPPORTED_LIVE_TYPES.includes(source.type ?? -1),
    )
    if (index < 0) {
      sourceIndex.value = -1
      return
    }
    sourceIndex.value = index
    await loadGroups(index)
  } catch (e) {
    sources.value = []
    sourceIndex.value = -1
    sourceError.value = api.formatError(e)
  } finally {
    loadingSources.value = false
    if (reloadPending) {
      reloadPending = false
      void loadSources()
    }
  }
}

async function loadGroups(index: number) {
  const seq = ++groupSeq
  loadingGroups.value = true
  groupError.value = ''
  activeGroupKey.value = ''
  activeChannel.value = null
  activeUrlIndex.value = 0
  programs.value = []
  epgError.value = ''
  try {
    const list = await api.liveGroups(index)
    if (seq !== groupSeq) return
    groups.value = list.map((group) => ({ ...group, channels: group.channels ?? [] }))
    // 默认选中第一个公开分组；全部分组加密时选中第一个（展示密码锁）
    // 不 await EPG 请求：分组加载不应被节目单网络请求阻塞
    const first = groups.value.find((group) => !group.password) ?? groups.value[0]
    if (first) void selectGroup(first.key, false)
  } catch (e) {
    if (seq !== groupSeq) return
    groups.value = []
    groupError.value = api.formatError(e)
  } finally {
    if (seq === groupSeq) loadingGroups.value = false
  }
}

/* ---------------- 分组 / 密码 ---------------- */

async function promptPassword(group: LiveGroup) {
  try {
    const { value } = await ElMessageBox.prompt(
      `分组「${group.name}」已加密，请输入密码后查看频道`,
      '直播分组密码',
      {
        confirmButtonText: '解锁',
        cancelButtonText: '取消',
        inputType: 'password',
        inputPlaceholder: '分组密码',
      },
    )
    if ((value ?? '') !== group.password) {
      ui.notify('密码错误', 'error')
      return
    }
    unlocked.value = { ...unlocked.value, [group.key]: value }
    const first = group.channels?.[0]
    if (first) await selectChannel(first, false)
    ui.notify(`已解锁「${group.name}」`, 'success')
  } catch {
    /* 用户取消，停留在密码锁面板 */
  }
}

async function selectGroup(key: string, promptIfLocked = true) {
  const group = groups.value.find((item) => item.key === key)
  if (!group) return
  activeGroupKey.value = key
  activeChannel.value = null
  activeUrlIndex.value = 0
  programs.value = []
  epgError.value = ''
  if (group.password && unlocked.value[group.key] !== group.password) {
    if (promptIfLocked) await promptPassword(group)
    return
  }
  const first = group.channels?.[0]
  if (first) await selectChannel(first, false)
}

/* ---------------- 频道 / 多源 / EPG ---------------- */

/** EPG 地址：优先 lives 组级 epg → m3u 组级 epg → 频道 epg（仅接受 http(s) URL） */
function resolveEpgUrl(channel: LiveChannel): string {
  const sourceEpg = currentSource.value?.epg
  if (sourceEpg && /^https?:\/\//i.test(sourceEpg)) return sourceEpg
  const groupEpg = activeGroup.value?.epg
  if (groupEpg && /^https?:\/\//i.test(groupEpg)) return groupEpg
  if (channel.epg && /^https?:\/\//i.test(channel.epg)) return channel.epg
  return ''
}

async function selectChannel(channel: LiveChannel, fromClick = true) {
  activeChannel.value = channel
  activeUrlIndex.value = 0
  programs.value = []
  epgError.value = ''
  if (fromClick) {
    if (channel.urls?.length) {
      ui.notify('直播播放器待接入：已展示直链与请求头', 'info')
    } else {
      ui.notify('该频道没有可用播放地址', 'warning')
    }
  }

  const epgUrl = resolveEpgUrl(channel)
  if (!epgUrl) return
  const seq = ++epgSeq
  loadingEpg.value = true
  try {
    const list = await api.liveEpg({
      epgUrl,
      // {name} 模板按频道名替换（与参考实现 LivePlayActivity 一致）
      channel: channel.name,
      date: today(),
    })
    if (seq !== epgSeq) return
    // XMLTV 往往是整站节目单：优先过滤当前频道（tvg-id / 频道名），无匹配时退回完整列表
    const wanted = [channel.epg, channel.name]
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
    if (seq === epgSeq) loadingEpg.value = false
  }
}

function onChannelClick(channel: LiveChannel) {
  const groupKey = channel.groupKey || activeGroupKey.value
  if (!groupKey) {
    void selectChannel(channel, true)
    return
  }
  // 频道详情是 /live 的子路由；用 name + params 让 vue-router 负责编码
  void router.push({
    name: 'live-detail',
    params: { group: groupKey, channel: channel.name },
  })
}

/** 换源：循环当前频道的 urls 并展示源名 */
function cycleSource() {
  if (urlOptions.value.length < 2) return
  activeUrlIndex.value = (activeUrlIndex.value + 1) % urlOptions.value.length
  ui.notify(`已切换到 ${currentSourceName.value}`, 'info')
}

function reloadGroups() {
  if (sourceIndex.value >= 0) void loadGroups(sourceIndex.value)
}

function reloadEpg() {
  if (activeChannel.value) void selectChannel(activeChannel.value, false)
}

function goSettings() {
  void router.push('/settings')
}

onMounted(() => {
  if (config.initialized) void loadSources()
})

// 配置初始化完成 / 切换启用源后重新加载直播源
watch(
  () => [config.initialized, config.enabled] as const,
  ([ready], prev) => {
    if (!ready) return
    const changed = !prev || prev[0] !== ready || prev[1] !== config.enabled
    if (changed) void loadSources()
  },
  { immediate: true },
)
</script>

<template>
  <PageScroll>
    <div class="page live-page">
    <!-- 演示模式提示 -->
    <div v-if="demoMode" class="live-notice" role="note">
      <span>演示模式：直播分组/频道来自 mock 数据；Tauri 桌面端使用当前订阅的真实 lives 源。</span>
    </div>

    <SkeletonGrid v-if="waitingConfig || loadingSources || loadingGroups" variant="card" :count="6" />

    <ErrorState v-else-if="sourceError" :message="sourceError" title="直播源加载失败" @retry="loadSources" />

    <EmptyState
      v-else-if="noLiveSource"
      title="当前订阅没有可用的直播源"
      desc="lives 中没有 type=0（txt/m3u 订阅）或 type=3（js spider）的源；可在设置页更换订阅或启用其它地址。"
      action-text="去设置更换订阅"
      @action="goSettings"
    />

    <ErrorState
      v-else-if="groupError"
      :message="groupError"
      title="直播分组加载失败"
      retry-text="重试"
      @retry="reloadGroups"
    />

    <EmptyState
      v-else-if="!groups.length"
      title="暂无直播分组"
      :desc="
        currentSource?.name
          ? `直播源「${currentSource.name}」没有解析出分组：订阅可能为空，或 type3 脚本未实现 live()。`
          : '当前订阅没有解析出直播分组。'
      "
    />

    <div v-else class="live-body">
      <LiveGroupNav
        :groups="groups"
        :active="activeGroupKey"
        :loading="loadingGroups"
        @select="selectGroup"
      />

      <section class="live-main" aria-label="频道列表">
        <!-- 密码分组：未解锁前不展示频道 -->
        <div v-if="activeGroupLocked" class="lock-panel">
          <SolarIcon class="lock-icon" name="lock" />
          <p class="lock-title">分组「{{ activeGroup?.name }}」需要密码</p>
          <p class="lock-desc">输入分组密码后才能查看频道；密码仅保存在本次会话。</p>
          <el-button
            type="primary"
            size="small"
            @click="activeGroup && promptPassword(activeGroup)"
          >
            输入密码
          </el-button>
        </div>

        <EmptyState
          v-else-if="!channels.length"
          title="该分组暂无频道"
          desc="直播源解析成功但分组为空，可尝试刷新或更换直播源。"
        />

        <div v-else class="channel-grid">
          <ChannelCard
            v-for="(channel, index) in channels"
            :key="`${channel.groupKey || activeGroupKey}-${channel.name}-${index}`"
            :channel="channel"
            @select="onChannelClick"
          />
        </div>
      </section>
    </div>

    <!-- 当前频道：播放器占位时展示直链 / 请求头 / 换源 -->
    <section v-if="activeChannel && !activeGroupLocked" class="live-play" aria-label="直播地址">
      <div class="play-head">
        <span class="play-channel">{{ activeChannel.name }}</span>
        <span v-if="activeGroup?.name" class="play-group">（{{ activeGroup.name }}）</span>
        <span class="play-spacer" />
        <el-button
          v-if="urlOptions.length > 1"
          size="small"
          @click="cycleSource"
        >
          <template #icon><SolarIcon name="refresh" /></template>换源（{{ activeUrlIndex + 1 }}/{{ urlOptions.length }} · {{ currentSourceName }}）
        </el-button>
        <el-button v-if="resolveEpgUrl(activeChannel)" size="small" text @click="reloadEpg">
          刷新节目单
        </el-button>
      </div>
      <p class="play-url mono" :title="currentPlayUrl">
        {{ currentPlayUrl || '该频道没有可用播放地址' }}
      </p>
      <p v-if="currentHeaders.length" class="play-meta">
        请求头：{{ currentHeaders.join(' · ') }}
      </p>
      <p v-if="currentReferer" class="play-meta">Referer：{{ currentReferer }}</p>
      <p class="play-hint">播放器待接入：当前仅展示直播地址与请求头，不进行播放。</p>
    </section>

    <EpgStrip
      v-if="!waitingConfig && !sourceError && !noLiveSource && groups.length"
      :channel="activeChannel"
      :programs="programs"
      :loading="loadingEpg"
      :error="epgError"
      @retry="reloadEpg"
    />
    </div>
  </PageScroll>
</template>

<style scoped>
.live-notice {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
  padding: var(--space-2) var(--space-3);
  border-left: 3px solid var(--info);
  border-radius: var(--radius-sm);
  background: var(--info-weak);
  color: var(--text-regular);
  font-size: 12px;
}

.live-body {
  display: flex;
  align-items: stretch;
  gap: var(--space-4);
  min-height: 260px;
}

.live-main {
  flex: 1;
  min-width: 0;
}

.channel-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: var(--space-3);
}

.lock-panel {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
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

.live-play {
  margin-top: var(--space-4);
  padding: var(--space-3) var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  background: var(--surface);
}

.play-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.play-channel {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.play-group {
  font-size: 12px;
  color: var(--text-faint);
}

.play-spacer {
  flex: 1;
}

.play-url {
  margin: var(--space-2) 0 0;
  overflow: hidden;
  font-size: 12px;
  color: var(--text-regular);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mono {
  font-family: var(--font-mono);
}

.play-meta {
  margin: var(--space-1) 0 0;
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.play-hint {
  margin: var(--space-2) 0 0;
  font-size: 11px;
  color: var(--text-faint);
}

@media (max-width: 860px) {
  .live-body {
    gap: var(--space-3);
  }
  .channel-grid {
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  }
}
</style>
