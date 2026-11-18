<!-- 设置页 · 网盘账号（W13）：115 / 夸克 / 迅雷 协议登录 + 阿里/UC/PikPak 网页授权兜底 -->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import { storeToRefs } from 'pinia'
import * as api from '@/api'
import { useCloudAccountsStore } from '@/store/cloudAccounts'
import type {
  PanAccountStatus,
  PanAccountView,
  PanLoginPollResult,
  PanLoginProvider,
  PanQrTicket,
} from '@/types'

/** provider 展示元信息（顺序即卡片顺序） */
const PROVIDER_META: { provider: string; name: string; desc: string }[] = [
  { provider: 'p115', name: '115 网盘', desc: '扫码登录；用于 115 分享解析与播放' },
  { provider: 'quark', name: '夸克网盘', desc: '扫码登录；用于夸克分享解析' },
  { provider: 'xunlei', name: '迅雷云盘', desc: '设备码授权；用于迅雷/PikPak 分享解析' },
  { provider: 'ali', name: '阿里云盘', desc: '网页扫码（官方登录页二维码）' },
  { provider: 'uc', name: 'UC 网盘', desc: '扫码登录；用于 UC 分享解析' },
  { provider: 'pikpak', name: 'PikPak', desc: '网页扫码（官方登录页二维码）' },
]

const STATUS_TEXT: Record<PanAccountStatus, string> = {
  connected: '已连接',
  expired: '已过期',
  not_logged_in: '未登录',
}

const STATUS_TAG: Record<PanAccountStatus, 'success' | 'warning' | 'info'> = {
  connected: 'success',
  expired: 'warning',
  not_logged_in: 'info',
}

const props = defineProps<{
  /** 打开时聚焦的 provider（云盘首页过期胶囊入口） */
  focusProvider?: string
}>()

const cloudAccounts = useCloudAccountsStore()
const { providers, accounts } = storeToRefs(cloudAccounts)
const loading = ref(false)
const error = ref('')
const highlighted = ref('')
let highlightTimer: number | undefined

/* ---------------- 账号状态 ---------------- */

function capabilityOf(provider: string): PanLoginProvider | undefined {
  return providers.value.find((item) => item.provider === provider)
}

function accountOf(provider: string): PanAccountView | undefined {
  return accounts.value.find((item) => item.provider === provider)
}

function statusOf(provider: string): PanAccountStatus {
  return accountOf(provider)?.status ?? 'not_logged_in'
}

function statusText(provider: string): string {
  return STATUS_TEXT[statusOf(provider)]
}

function statusTag(provider: string): 'success' | 'warning' | 'info' {
  return STATUS_TAG[statusOf(provider)]
}

/** 账号展示行：昵称（有就优先）+ 掩码标识 */
function accountLine(provider: string): string {
  const account = accountOf(provider)
  if (!account) return '未登录'
  const name = account.display_name?.trim()
  const masked = account.masked_id?.trim()
  if (name && masked) return `${name}（${masked}）`
  return name || masked || account.account_id
}

/** Unix 秒 → 本地时间文案 */
function formatTime(ts: number): string {
  if (!ts) return '—'
  return new Date(ts * 1000).toLocaleString()
}

/* ---------------- 外部聚焦（云盘首页过期胶囊 → 重新登录） ---------------- */

/** 滚动到指定 provider 卡片并短暂高亮；账号列表加载完成后调用 */
async function focusCard(provider: string | undefined): Promise<void> {
  if (!provider) return
  await nextTick()
  const el = document.getElementById(`pan-card-${provider}`)
  if (!el) return
  el.scrollIntoView({ behavior: 'smooth', block: 'center' })
  highlighted.value = provider
  if (highlightTimer !== undefined) window.clearTimeout(highlightTimer)
  highlightTimer = window.setTimeout(() => {
    highlighted.value = ''
    highlightTimer = undefined
  }, 2600)
}

watch(
  () => props.focusProvider,
  (provider) => {
    if (provider) void focusCard(provider)
  },
)

async function load() {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    // 账号状态统一由 store 管理（W14 §7.3）
    await cloudAccounts.load(true)
    if (cloudAccounts.error) error.value = cloudAccounts.error
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    loading.value = false
  }
}

/* ---------------- 协议登录（扫码 / 设备码） ---------------- */

const loginVisible = ref(false)
const loginProvider = ref('')
const loginStarting = ref(false)
const loginError = ref('')
const ticket = ref<PanQrTicket | null>(null)
/** 倒计时剩余秒数 */
const remain = ref(0)
/** 最近一次轮询状态（waiting/scanned） */
const pollState = ref<'waiting' | 'scanned'>('waiting')
const loginBusy = ref(false)
let tickTimer: number | undefined
let pollTimer: number | undefined

const loginProviderName = computed(
  () => PROVIDER_META.find((item) => item.provider === loginProvider.value)?.name ?? loginProvider.value,
)

/** 未登录主按钮：有协议扫码走站内弹窗，否则走官方网页扫码（WebView） */
function primaryLogin(provider: string) {
  if (capabilityOf(provider)?.protocol) void startLogin(provider)
  else void webLogin(provider)
}

/** 登录方式文案（115/夸克/UC=扫码，迅雷=设备码） */
function actionText(provider: string): string {
  return provider === 'xunlei' ? '设备码登录' : '扫码登录'
}

function stopTimers() {
  if (tickTimer !== undefined) {
    window.clearInterval(tickTimer)
    tickTimer = undefined
  }
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
    pollTimer = undefined
  }
}

function startTimers() {
  stopTimers()
  tickTimer = window.setInterval(() => {
    if (remain.value > 0) remain.value -= 1
    if (remain.value === 0) {
      stopTimers()
      if (ticket.value) loginError.value = '二维码已过期，请刷新后重试'
    }
  }, 1000)
  pollTimer = window.setInterval(() => {
    void pollOnce()
  }, 2000)
}

async function startLogin(provider: string) {
  if (api.demoMode) return
  if (loginStarting.value) return
  loginProvider.value = provider
  loginVisible.value = true
  loginBusy.value = false
  loginError.value = ''
  ticket.value = null
  pollState.value = 'waiting'
  remain.value = 0
  await refreshTicket()
}

async function refreshTicket() {
  const provider = loginProvider.value
  if (!provider || loginStarting.value) return
  loginStarting.value = true
  loginError.value = ''
  try {
    const next = await api.panLoginStart(provider)
    ticket.value = next
    remain.value = next.expires_in
    if (next.expires_at > 0) {
      remain.value = Math.max(0, next.expires_at - Math.floor(Date.now() / 1000))
    }
    pollState.value = 'waiting'
    startTimers()
  } catch (e) {
    loginError.value = api.formatError(e)
  } finally {
    loginStarting.value = false
  }
}

/** 轮询一次登录状态；终态时关闭弹窗并刷新账号列表 */
async function pollOnce() {
  const current = ticket.value
  const provider = loginProvider.value
  if (!current || !provider || loginBusy.value) return
  loginBusy.value = true
  try {
    const result: PanLoginPollResult = await api.panLoginPoll(provider, current.ticket_id)
    if (result.state === 'scanned') {
      if (pollState.value !== 'scanned') {
        pollState.value = 'scanned'
        ElMessage.success('已扫码，请在手机上确认')
      }
    } else if (result.state === 'success') {
      stopTimers()
      ElMessage.success(`${loginProviderName.value} 登录成功`)
      loginVisible.value = false
      ticket.value = null
      await load()
    } else if (result.state === 'expired') {
      stopTimers()
      ticket.value = null
      loginError.value = '二维码已过期，请刷新后重试'
    } else if (result.state === 'failure') {
      stopTimers()
      loginError.value = result.message
    } else {
      pollState.value = 'waiting'
    }
  } catch (e) {
    // 单次轮询失败不打断会话（网络抖动/长轮询超时都按继续处理）
    pollState.value = pollState.value === 'scanned' ? 'scanned' : 'waiting'
    void e
  } finally {
    loginBusy.value = false
  }
}

async function closeLogin() {
  const current = ticket.value
  loginVisible.value = false
  stopTimers()
  ticket.value = null
  if (current && api.isTauri) {
    try {
      await api.panLoginCancel(current.ticket_id)
    } catch {
      // 取消失败不影响关闭
    }
  }
}

/** 在系统浏览器打开扫码页 / 设备授权页 */
async function openExternal(url: string) {
  if (!url) return
  try {
    await api.pluginOpenUrl(url)
  } catch (e) {
    error.value = api.formatError(e)
  }
}

/* ---------------- 网页授权兜底 ---------------- */

const webLoginBusy = ref('')

async function webLogin(provider: string) {
  if (api.demoMode || webLoginBusy.value) return
  const meta = PROVIDER_META.find((item) => item.provider === provider)
  webLoginBusy.value = provider
  error.value = ''
  try {
    const account = await api.panOpenWebLogin(provider, 300)
    if (account) {
      ElMessage.success(`${meta?.name ?? provider} 网页授权成功`)
      await load()
    } else {
      ElMessage.info('已取消网页授权')
    }
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    webLoginBusy.value = ''
  }
}

/* ---------------- 验证 / 退出 / 手动凭据 ---------------- */

const verifyBusy = ref('')

async function verify(provider: string) {
  if (api.demoMode || verifyBusy.value) return
  verifyBusy.value = provider
  try {
    const ok = await api.panCredentialsVerify(provider)
    if (ok) ElMessage.success('凭据有效')
    else ElMessage.warning('凭据缺失或已过期，请重新登录')
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    verifyBusy.value = ''
  }
}

async function logout(provider: string) {
  const meta = PROVIDER_META.find((item) => item.provider === provider)
  try {
    await ElMessageBox.confirm(
      `退出后将从本机删除 ${meta?.name ?? provider} 的登录凭据，相关分享解析会恢复为未登录状态。`,
      '退出账号',
      { type: 'warning', confirmButtonText: '退出', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  try {
    const count = await api.panCredentialsClear(provider)
    ElMessage.success(count > 0 ? `已退出 ${count} 个账号` : '没有可退出的账号')
    await load()
  } catch (e) {
    error.value = api.formatError(e)
  }
}

const manualVisible = ref(false)
const manualProvider = ref('p115')
const manualSaving = ref(false)
const manualForm = ref({ cookie: '', token: '', refreshToken: '', displayName: '' })

/** 各网盘手动凭据的关键 Cookie/Token 字段说明（W14 补齐：扫码同源字段） */
const MANUAL_FIELD_HINT: Record<string, string> = {
  p115: '115：粘贴整段 Cookie，关键字段 UID / CID / SEID（115.com 登录后 F12 → Network 任一请求的 Cookie）。',
  quark: '夸克：关键 Cookie 字段 __pus / __puus（pan.quark.cn 登录后复制整段 Cookie）。',
  uc: 'UC：关键 Cookie 字段 __pus / __puus（drive.uc.cn 登录后复制整段 Cookie）。',
  xunlei: '迅雷：填 Token（access_token）与 refresh_token（设备码授权后获得），Cookie 可留空。',
  ali: '阿里：填 refresh_token（openapi 授权）或整段 Cookie（alipan.com）。',
  pikpak: 'PikPak：填 Token（access_token）与 refresh_token，Cookie 可留空。',
}
const manualHint = computed(
  () => MANUAL_FIELD_HINT[manualProvider.value] ?? '从浏览器或第三方工具导出后粘贴；至少填写 Cookie 或 Token。',
)
const manualCookiePlaceholder = computed(() => {
  switch (manualProvider.value) {
    case 'quark':
    case 'uc':
      return 'Cookie（关键 __pus=…; __puus=…）'
    case 'p115':
      return 'Cookie（如 UID=…; CID=…; SEID=…）'
    default:
      return 'Cookie（整段粘贴，可选）'
  }
})

function openManual(provider: string) {
  manualProvider.value = provider
  manualForm.value = { cookie: '', token: '', refreshToken: '', displayName: '' }
  manualVisible.value = true
}

async function saveManual() {
  if (manualSaving.value) return
  const form = manualForm.value
  if (!form.cookie.trim() && !form.token.trim()) {
    ElMessage.warning('请至少填写 Cookie 或 Token')
    return
  }
  manualSaving.value = true
  try {
    await api.panLoginManual({
      provider: manualProvider.value,
      cookie: form.cookie.trim() || undefined,
      token: form.token.trim() || undefined,
      refreshToken: form.refreshToken.trim() || undefined,
      displayName: form.displayName.trim() || undefined,
    })
    manualVisible.value = false
    ElMessage.success('凭据已保存（本机加密）')
    await load()
  } catch (e) {
    error.value = api.formatError(e)
  } finally {
    manualSaving.value = false
  }
}

onMounted(() => {
  void load()
})

onBeforeUnmount(() => {
  if (highlightTimer !== undefined) window.clearTimeout(highlightTimer)
  stopTimers()
})
</script>

<template>
  <section
    class="settings-section"
    aria-labelledby="settings-pan"
  >
    <header class="section-head">
      <h2 id="settings-pan" class="section-title">网盘账号</h2>
      <p class="section-desc">
        登录网盘账号后，云盘分享可直接解析播放（115 / 夸克 / UC 扫码、迅雷设备码；
        阿里 / PikPak 走官方网页扫码）。凭据仅保存在本机。
      </p>
    </header>

    <HintBar v-if="api.demoMode">
      浏览器演示模式：网盘登录只在桌面端（Tauri）可用。
    </HintBar>

    <HintBar v-if="error" variant="error">
      {{ error }}
    </HintBar>

    <div class="pan-grid">
      <div
        v-for="meta in PROVIDER_META"
        :id="`pan-card-${meta.provider}`"
        :key="meta.provider"
        class="pan-card"
        :class="{
          'is-connected': statusOf(meta.provider) === 'connected',
          'is-highlighted': highlighted === meta.provider,
        }"
      >
        <div class="pan-card-head">
          <div class="pan-card-headmain">
            <div class="pan-card-title">
              <strong>{{ meta.name }}</strong>
              <el-tag
                size="small"
                :type="statusTag(meta.provider)"
                effect="plain"
              >
                {{ statusText(meta.provider) }}
              </el-tag>
            </div>
            <span class="pan-card-desc">{{ meta.desc }}</span>
          </div>
          <div class="pan-card-actions">
            <template v-if="statusOf(meta.provider) === 'connected'">
              <el-button
                size="small"
                :loading="verifyBusy === meta.provider"
                :disabled="api.demoMode"
                @click="verify(meta.provider)"
              >
                <template #icon><SolarIcon name="check-circle" /></template>
                检查状态
              </el-button>
              <el-button
                size="small"
                :disabled="api.demoMode"
                @click="logout(meta.provider)"
              >
                <template #icon><SolarIcon name="trash-bin-minimalistic" /></template>
                登出
              </el-button>
            </template>
            <template v-else>
              <el-button
                size="small"
                type="primary"
                :disabled="api.demoMode"
                @click="primaryLogin(meta.provider)"
              >
                <template #icon><SolarIcon name="refresh-circle" /></template>
                {{ actionText(meta.provider) }}
              </el-button>
              <el-button
                size="small"
                :disabled="api.demoMode"
                @click="openManual(meta.provider)"
              >
                手动凭据
              </el-button>
            </template>
          </div>
        </div>

        <div class="pan-card-body">
          <span class="status-label">账号：{{ accountLine(meta.provider) }}</span>
          <span class="status-label">
            更新：{{ formatTime(accountOf(meta.provider)?.updated_at ?? 0) }}
          </span>
        </div>

      </div>
    </div>

    <p class="section-note">
      凭据仅本机加密保存（Windows DPAPI），不上传、不写日志、不导出；
      Cookie/Token 不会回传到界面。
    </p>

    <!-- 协议登录弹窗：二维码 / 设备码 -->
    <el-dialog
      v-model="loginVisible"
      align-center
      append-to-body
      :title="`${loginProviderName} 登录`"
      width="380px"
      :close-on-click-modal="false"
      @closed="closeLogin"
    >
      <div class="pan-login">
        <template v-if="loginError">
          <p class="status-error">
            ⚠ 登录失败：{{ loginError }}
          </p>
          <div class="pan-login-actions">
            <el-button
              type="primary"
              :loading="loginStarting"
              @click="refreshTicket"
            >
              重试
            </el-button>
            <el-button @click="loginVisible = false">
              关闭
            </el-button>
          </div>
        </template>
        <template v-else-if="ticket">
          <!-- 115 / 扫码型：官方二维码图片 -->
          <img
            v-if="ticket.kind === 'qr_code' && (ticket.image_url || ticket.qr_image)"
            class="pan-qr"
            :src="ticket.image_url || ticket.qr_image"
            alt="登录二维码"
          />
          <!-- 迅雷 / 设备码型：手机迅雷扫码授权（本地渲染二维码）+ 用户码兜底 -->
          <template v-else-if="ticket.kind === 'device_code'">
            <img
              v-if="ticket.qr_image"
              class="pan-qr"
              :src="ticket.qr_image"
              alt="用手机迅雷扫码授权"
            />
            <p class="pan-device-code">{{ ticket.content }}</p>
            <p class="status-label">
              用手机迅雷 App 扫码并在手机上确认授权；也可以打开授权页输入上面的用户码。
            </p>
            <el-button
              type="primary"
              @click="openExternal(ticket.image_url || ticket.content)"
            >
              打开授权页
            </el-button>
          </template>
          <!-- 夸克：扫码页链接（手机扫码） -->
          <template v-else>
            <p class="status-label">用手机扫描下方链接打开的二维码页面：</p>
            <el-button
              type="primary"
              @click="openExternal(ticket.content)"
            >
              打开扫码页
            </el-button>
          </template>
          <p class="pan-countdown">
            剩余 {{ Math.max(0, remain) }} 秒
            <span v-if="pollState === 'scanned'" class="pan-scanned">已扫码，请在手机上确认</span>
          </p>
        </template>
        <p v-else class="status-label">正在获取登录票据…</p>
      </div>
      <template #footer>
        <el-button :disabled="loginStarting" @click="closeLogin">取消</el-button>
        <el-button
          type="primary"
          :disabled="loginStarting"
          @click="refreshTicket"
        >
          刷新{{ actionText(loginProvider) }}
        </el-button>
      </template>
    </el-dialog>

    <!-- 手动凭据弹窗 -->
    <el-dialog
      v-model="manualVisible"
      align-center
      append-to-body
      title="手动填写凭据"
      width="460px"
    >
      <div class="manual-form">
        <p class="status-label">{{ manualHint }}</p>
        <el-input
          v-model="manualForm.displayName"
          placeholder="显示名称（可选，如：我的 115）"
        />
        <el-input
          v-model="manualForm.cookie"
          type="textarea"
          :rows="3"
          :placeholder="manualCookiePlaceholder"
        />
        <el-input
          v-model="manualForm.token"
          placeholder="Token / access_token（可选）"
        />
        <el-input
          v-model="manualForm.refreshToken"
          placeholder="refresh_token（可选）"
        />
      </div>
      <template #footer>
        <el-button @click="manualVisible = false">取消</el-button>
        <el-button
          type="primary"
          :loading="manualSaving"
          @click="saveManual"
        >
          保存
        </el-button>
      </template>
    </el-dialog>
  </section>
</template>

<style scoped>
.pan-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: var(--space-3);
  margin-top: var(--space-3);
}

.pan-card.is-highlighted {
  box-shadow: 0 0 0 2px var(--accent);
  animation: pan-card-pulse 1.1s ease-in-out 2;
}

@keyframes pan-card-pulse {
  0%,
  100% {
    box-shadow: 0 0 0 2px var(--accent);
  }
  50% {
    box-shadow: 0 0 0 4px var(--accent-weak);
  }
}

.pan-card {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-3);
  border: 1px solid var(--el-border-color);
  border-radius: var(--radius-md);
  background: var(--el-fill-color-lighter);
}

.pan-card.is-connected {
  border-color: var(--el-color-success-light-5);
}

.pan-card-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-2);
}

.pan-card-headmain {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.pan-card-title {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.pan-card-desc {
  font-size: 12px;
  color: var(--text-muted);
}

.pan-card-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.pan-card-actions {
  display: flex;
  flex-shrink: 0;
  gap: 6px;
}

.pan-login {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-3);
  text-align: center;
}

.pan-qr {
  width: 200px;
  height: 200px;
  image-rendering: pixelated;
  border: 1px solid var(--el-border-color);
  border-radius: var(--radius-md);
  background: #fff;
}

.pan-device-code {
  margin: 0;
  font-family: var(--font-mono, monospace);
  font-size: 26px;
  font-weight: 700;
  letter-spacing: 4px;
  word-break: break-all;
}

.pan-login-actions {
  display: flex;
  gap: 8px;
  justify-content: center;
}

.pan-countdown {
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--text-muted);
}

.pan-scanned {
  color: var(--el-color-success);
}

.manual-form {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
</style>