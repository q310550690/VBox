<!-- 设置页 · 订阅管理分区（frontend-design.md §6.8）：迁移旧 Settings.vue 的全部订阅行为 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessageBox } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'
import HintBar from '@/components/HintBar.vue'
import * as api from '@/api'
import { useConfigStore } from '@/store/config'
import { describeSiteStats } from '@/utils/siteSupport'
import type { ConfigLine, SiteStats, Subscription } from '@/types'

const store = useConfigStore()

/** 当前线路的源能力统计（本机 / jar / Python / 未适配） */
const activeStats = computed<SiteStats>(() => store.siteStats)
/** 当前线路所有站点都是 jar（纯 jar 线路提示条件） */
const activeLinePureJar = computed(
  () => activeStats.value.total > 0 && activeStats.value.jar === activeStats.value.total,
)
/** 当前线路没有本机可用源（纯 jar / jar+Python 混合等） */
const activeLineNoLocal = computed(
  () => activeStats.value.total > 0 && activeStats.value.local === 0,
)

/** 某条线路的源能力文案（未切换过的线路没有缓存，提示切换后再统计） */
function statsTextOf(lineUrl: string): string {
  const stats = store.lineStatsOf(lineUrl)
  return stats ? describeSiteStats(stats) : '切换后统计源能力'
}

/** 已知该线路没有本机可用源（纯 jar / jar+未适配）→ 文案标警示色 */
function lineNoLocal(lineUrl: string): boolean {
  const stats = store.lineStatsOf(lineUrl)
  return Boolean(stats && stats.total > 0 && stats.local === 0)
}

/** 新增订阅表单（名称与链接双必填） */
const name = ref('')
const url = ref('')
const adding = ref(false)
/** 「重新加载启用源」按钮的加载态（调用 reload_enabled，默认强制刷新） */
const reloading = ref(false)

const canAdd = computed(() => name.value.trim().length > 0 && url.value.trim().length > 0)

/* ---------------- 导入 / 导出 / 定时刷新 / 编辑 ---------------- */

const importing = ref(false)
const exporting = ref(false)
const savingRefresh = ref(false)
const REFRESH_OPTIONS = [
  { value: 0, label: '关闭' },
  { value: 15, label: '每 15 分钟' },
  { value: 30, label: '每 30 分钟' },
  { value: 60, label: '每 60 分钟' },
  { value: 180, label: '每 3 小时' },
]

async function onImport() {
  if (importing.value) return
  importing.value = true
  try {
    await store.importSubscriptionsFromFile()
  } finally {
    importing.value = false
  }
}

async function onExport() {
  if (exporting.value) return
  exporting.value = true
  try {
    await store.exportSubscriptionsToFile()
  } finally {
    exporting.value = false
  }
}

async function onRefreshChange(value: string | number) {
  if (savingRefresh.value) return
  savingRefresh.value = true
  try {
    await store.setRefreshMinutes(Number(value) || 0)
  } finally {
    savingRefresh.value = false
  }
}

/** 正在编辑的订阅（null = 对话框关闭） */
const editing = ref<Subscription | null>(null)
const editName = ref('')
const editUrl = ref('')
const savingEdit = ref(false)

function onEdit(sub: Subscription) {
  editing.value = sub
  editName.value = sub.name
  editUrl.value = sub.url
}

function closeEdit() {
  editing.value = null
}

async function onSaveEdit() {
  if (!editing.value || savingEdit.value) return
  savingEdit.value = true
  try {
    await store.updateSubscription(editing.value.url, editName.value, editUrl.value)
    closeEdit()
  } catch {
    // 错误提示已由 store 统一 toast；保留对话框便于修改
  } finally {
    savingEdit.value = false
  }
}

/** 添加订阅：成功后清空表单，失败保留输入（错误提示由 store 统一 toast） */
async function onAdd() {
  const n = name.value.trim()
  const u = url.value.trim()
  if (!n || !u || adding.value) return
  adding.value = true
  try {
    await store.addSubscription(n, u)
    if (store.subscriptions.some((sub) => sub.url === u)) {
      name.value = ''
      url.value = ''
    }
  } finally {
    adding.value = false
  }
}

/** 展开 / 收起订阅地址 */
function onToggle(sub: Subscription) {
  void store.toggleExpand(sub.name, sub.url)
}

/** 线路所属订阅 URL（传给后端 set_enabled，保持多仓 kind=multi + sourceUrl） */
function sourceOf(lineUrl: string): string | undefined {
  for (const sub of store.subscriptions) {
    if (sub.url === lineUrl) return sub.url
    const group = store.addressGroups[sub.url]
    if (group?.lines.some((line) => line.url === lineUrl)) return sub.url
  }
  return store.sourceUrl || store.loaded?.sourceUrl || undefined
}

/** 地址 el-radio 单选 → 启用该线路（带所属订阅 sourceUrl） */
function onAddressChange(val: string | number | boolean | undefined) {
  if (typeof val === 'string' && val && val !== store.enabled) {
    void store.setEnabled(val, sourceOf(val))
  }
}

/** 取某订阅的线路列表（get_addresses 的 lines；未展开时为空） */
function addressesOf(sub: Subscription): ConfigLine[] {
  return store.addressGroups[sub.url]?.lines ?? []
}

/** 订阅形态说明：单仓 / 多仓 + 线路数 */
function kindLabel(sub: Subscription): string {
  const group = store.addressGroups[sub.url]
  if (!group) return '地址加载中…'
  return group.kind === 'multi' ? `多仓 · ${group.lines.length} 条线路` : '单仓'
}

/** 是否多仓：只有多仓才需要展开/收起；单仓直接平铺（默认展开） */
function isMulti(sub: Subscription): boolean {
  return store.addressGroups[sub.url]?.kind === 'multi'
}

/** 线路列表是否显示：单仓恒显示，多仓按展开状态 */
function isExpanded(sub: Subscription): boolean {
  if (!isMulti(sub)) return true
  return Boolean(store.expanded[sub.url])
}

/** 底部展开条文案：展开全部 N 条线路 / 收起 */
function toggleLabel(sub: Subscription): string {
  if (store.expanded[sub.url]) return '收起'
  const total = store.addressGroups[sub.url]?.lines.length ?? 0
  return total > 0 ? `展开全部 ${total} 条线路` : '展开线路'
}

/** 当前启用线路是否属于该订阅（单仓含自身；多仓含任一子线路 / sourceUrl 匹配） */
function isSubActive(sub: Subscription): boolean {
  if (store.enabled === sub.url) return true
  if (store.sourceUrl === sub.url) return true
  if (store.addressGroups[sub.url]?.lines.some((line) => line.url === store.enabled)) return true
  return store.loaded?.input === sub.url
}

/** 删除订阅：二次确认后才真正删除 */
async function onRemove(sub: Subscription) {
  try {
    await ElMessageBox.confirm(
      `删除订阅「${sub.name}」后，其下地址将不可用。确认删除？`,
      '删除订阅',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
    await store.removeSubscription(sub.url)
  } catch {
    /* 用户取消，或删除失败（失败已由 store 提示） */
  }
}

/** 重新加载当前启用源（reload_enabled + 强制刷新） */
async function onReload() {
  if (reloading.value || !store.enabled) return
  reloading.value = true
  try {
    await store.reload(true)
  } finally {
    reloading.value = false
  }
}

/** 直达设置页时由本组件兜底初始化（store.init 内部有 loading/initialized 幂等保护） */
onMounted(() => {
  if (!store.initialized && !store.loading) void store.init()
})
</script>

<template>
  <section
    class="settings-section"
    aria-labelledby="settings-subscription"
  >
    <header class="section-head">
      <h2
        id="settings-subscription"
        class="section-title"
      >订阅管理</h2>
      <p class="section-desc">添加订阅源，选择一个地址作为当前启用源。</p>
    </header>

    <HintBar v-if="store.demoMode">
      浏览器演示模式：以下订阅与地址均为演示数据，不会读写本地配置。
    </HintBar>
    <HintBar
      v-if="store.error"
      variant="error"
    >订阅初始化失败：{{ store.error }}</HintBar>

    <div class="add-form">
      <el-input
        v-model="name"
        class="name-input"
        placeholder="订阅名称（必填）"
        maxlength="40"
        clearable
        aria-label="订阅名称"
      />
      <el-input
        v-model="url"
        class="url-input"
        placeholder="订阅链接（必填，http(s)://）"
        clearable
        aria-label="订阅链接"
        @keyup.enter="onAdd"
      />
      <el-button
        type="primary"
        :disabled="!canAdd || adding"
        :loading="adding"
        @click="onAdd"
      >
        <template #icon>
          <SolarIcon name="add" />
        </template>
        添加
      </el-button>
    </div>
    <p
      v-if="!canAdd"
      class="form-tip"
    >名称与链接均为必填，填写完整后才可添加。</p>

    <div class="sub-toolbar">
      <el-button :loading="importing" :disabled="api.demoMode" @click="onImport">
        <template #icon><SolarIcon name="cloud-upload" /></template>
        导入订阅
      </el-button>
      <el-button
        :loading="exporting"
        :disabled="api.demoMode || !store.subscriptions.length"
        @click="onExport"
      >
        <template #icon><SolarIcon name="download" /></template>
        导出订阅
      </el-button>
      <span class="refresh-control">
        <SolarIcon name="refresh" :size="14" aria-hidden="true" />
        <span class="switch-label">定时刷新</span>
        <el-select
          :model-value="store.refreshMinutes"
          :disabled="api.demoMode || savingRefresh"
          style="width: 128px"
          aria-label="定时刷新间隔"
          @change="onRefreshChange"
        >
          <el-option
            v-for="option in REFRESH_OPTIONS"
            :key="option.value"
            :label="option.label"
            :value="option.value"
          />
        </el-select>
      </span>
    </div>

    <div
      v-if="!store.subscriptions.length && !store.loading"
      class="empty-hint"
    >
      <p class="empty-title">还没有订阅</p>
      <p class="empty-desc">在上方填写名称与链接后点击「添加」。</p>
    </div>
    <p
      v-else-if="store.loading && !store.subscriptions.length"
      class="loading-hint"
    >
      订阅数据加载中…
    </p>
    <ul
      v-else
      class="sub-list"
    >
      <li
        v-for="sub in store.subscriptions"
        :key="sub.url"
        class="sub-item"
      >
        <div class="sub-head">
          <div class="sub-main">
            <div class="sub-title-line">
              <span class="sub-name ellipsis-1">{{ sub.name }}</span>
              <el-tag
                size="small"
                type="info"
                effect="plain"
              >{{ kindLabel(sub) }}</el-tag>
              <el-tag
                v-if="isSubActive(sub)"
                size="small"
                type="success"
                effect="dark"
              >
                当前启用
              </el-tag>
            </div>
            <span
              class="sub-url ellipsis-1"
              :title="sub.url"
            >{{ sub.url }}</span>
          </div>
          <el-button
            text
            :aria-label="`编辑订阅 ${sub.name}`"
            @click="onEdit(sub)"
          >
            <template #icon>
              <SolarIcon name="file-pen" />
            </template>
            编辑
          </el-button>
          <el-button
            type="danger"
            text
            :aria-label="`删除订阅 ${sub.name}`"
            @click="onRemove(sub)"
          >
            <template #icon>
              <SolarIcon name="trash-bin-minimalistic" />
            </template>
            删除
          </el-button>
        </div>

        <div
          v-if="isExpanded(sub)"
          class="address-box"
        >
          <!-- 当前线路全是 jar 时给出明确切换提示 -->
          <p
            v-if="activeLineNoLocal && isSubActive(sub)"
            class="line-warn"
          >
            {{
              activeLinePureJar
                ? '该线路为 Android jar 源，桌面端后续支持，请切换线路'
                : '该线路暂无本机可用源，请切换线路'
            }}
          </p>
          <p
            v-if="!addressesOf(sub).length"
            class="address-empty"
          >该订阅暂无可用地址（或正在加载）。</p>
          <el-scrollbar
            v-else
            class="address-scroll"
            max-height="240px"
          >
            <el-radio-group
              class="address-list"
              :model-value="store.enabled"
              :aria-label="`${sub.name} 的地址列表`"
              @change="onAddressChange"
            >
              <el-radio
                v-for="addr in addressesOf(sub)"
                :key="addr.url"
                :value="addr.url"
                class="address-item"
              >
                <span class="address-line">
                  <span class="address-name ellipsis-1">{{ addr.name || addr.url }}</span>
                  <el-tag
                    v-if="addr.url === store.enabled"
                    size="small"
                    type="success"
                    effect="plain"
                  >
                    当前线路
                  </el-tag>
                </span>
                <span
                  v-if="addr.name && addr.name !== addr.url"
                  class="address-url ellipsis-1"
                >
                  {{ addr.url }}
                </span>
                <!-- 源能力统计：切换过的线路显示“本机 N · jar N”；未切换过的提示切换后统计 -->
                <span
                  class="address-stats"
                  :class="{
                    'address-stats--muted': !store.lineStatsOf(addr.url),
                    'address-stats--warn': lineNoLocal(addr.url),
                  }"
                >
                  {{ statsTextOf(addr.url) }}
                </span>
              </el-radio>
            </el-radio-group>
          </el-scrollbar>
        </div>

        <!-- 底部展开 / 收起条（仅多仓；单仓默认全展开，不需要折叠） -->
        <footer
          v-if="isMulti(sub)"
          class="sub-foot"
        >
          <button
            type="button"
            class="sub-expand"
            :aria-label="store.expanded[sub.url] ? `收起 ${sub.name} 的线路` : `展开 ${sub.name} 的线路`"
            :aria-expanded="!!store.expanded[sub.url]"
            @click="onToggle(sub)"
          >
            <span
              class="expand-icon"
              :class="{ 'is-expanded': !!store.expanded[sub.url] }"
            >
              <SolarIcon
                name="alt-arrow-down"
                :size="14"
              />
            </span>
            <span>{{ toggleLabel(sub) }}</span>
          </button>
        </footer>
      </li>
    </ul>

    <div class="enabled-bar">
      <div class="enabled-info">
        <span class="enabled-label">当前线路</span>
        <span
          class="enabled-url ellipsis-1"
          :title="store.activeLine || store.enabled"
        >
          {{ store.activeLine || store.enabled || '未启用任何地址' }}
        </span>
        <span
          v-if="store.enabledSubscriptionName"
          class="enabled-sub"
        >
          来自：{{ store.enabledSubscriptionName }}
        </span>
        <span
          v-if="store.sourceUrl"
          class="enabled-sub ellipsis-1"
          :title="store.sourceUrl"
        >
          来源：{{ store.sourceUrl }}
        </span>
        <span
          v-if="store.activeLineName"
          class="enabled-sub"
        >
          线路：{{ store.activeLineName }}
        </span>
        <span
          v-if="store.enabled"
          class="enabled-sub"
        >
          {{ store.kind === 'multi' ? '线路合集（多仓）' : '单配置' }}
        </span>
        <span
          v-if="store.enabled"
          class="enabled-sub"
        >源能力：{{ describeSiteStats(activeStats) }}</span>
        <span
          v-if="store.enabled && activeLineNoLocal"
          class="enabled-warn"
        >
          {{
            activeLinePureJar
              ? '当前线路为 Android jar 源，后续支持（需 Android Host），建议切换线路'
              : '当前线路没有本机可用源，建议切换线路'
          }}
        </span>
        <!-- 加载元信息：解包格式 / 是否命中本地配置缓存（排障用，小字） -->
        <span
          v-if="store.enabled && store.format"
          class="enabled-meta"
        >
          格式：{{ store.format }} · {{ store.cacheHit ? '缓存命中' : '已联网拉取' }}
        </span>
      </div>
      <el-button
        :loading="reloading"
        :disabled="!store.enabled || reloading"
        @click="onReload"
      >
        <template #icon>
          <SolarIcon name="refresh" />
        </template>
        重新加载启用源
      </el-button>
    </div>
  
    <!-- 编辑订阅名称/链接 -->
    <el-dialog
      :model-value="editing !== null"
      title="编辑订阅"
      class="dlg-md"
      append-to-body
      align-center
      @update:model-value="(visible: boolean) => !visible && closeEdit()"
    >
      <div class="edit-field">
        <span class="switch-label">订阅名称</span>
        <el-input v-model="editName" maxlength="40" placeholder="订阅名称（必填）" aria-label="编辑订阅名称" />
      </div>
      <div class="edit-field">
        <span class="switch-label">订阅链接</span>
        <el-input v-model="editUrl" placeholder="订阅链接（必填）" aria-label="编辑订阅链接" />
      </div>
      <template #footer>
        <div class="edit-footer">
          <el-button @click="closeEdit">
            <template #icon><SolarIcon name="close" /></template>
            取消
          </el-button>
          <el-button
            type="primary"
            :loading="savingEdit"
            :disabled="!editName.trim() || !editUrl.trim()"
            @click="onSaveEdit"
          >
            <template #icon><SolarIcon name="check-circle" /></template>
            保存
          </el-button>
        </div>
      </template>
    </el-dialog>

</section>
</template>

<style scoped>
.add-form {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.name-input {
  width: 200px;
}

.url-input {
  flex: 1;
  min-width: 240px;
}

.sub-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin: var(--space-2) 0 var(--space-3);
}

.refresh-control {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  margin-left: auto;
  color: var(--text-muted);
  font-size: 12px;
}

.edit-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin-bottom: var(--space-3);
}

.edit-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
}

.form-tip {
  margin: var(--space-1) 0 0;
  font-size: 12px;
  color: var(--text-faint);
}

.empty-hint {
  margin-top: var(--space-3);
  padding: var(--space-4);
  border: 1px dashed var(--border);
  border-radius: var(--radius-md);
  text-align: center;
}

.empty-title {
  margin: 0;
  font-size: 13px;
  color: var(--text-regular);
}

.empty-desc {
  margin: var(--space-1) 0 0;
  font-size: 12px;
  color: var(--text-faint);
}

.loading-hint {
  margin: var(--space-3) 0 0;
  font-size: 12px;
  color: var(--text-faint);
}

.sub-list {
  margin: var(--space-3) 0 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.sub-item {
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
  overflow: hidden;
}

.sub-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-3);
}

.sub-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

/* 底部展开 / 收起条：与下载页剧集卡片底部同一套（整行居中、带上箭头旋转） */
.sub-foot {
  border-top: 1px solid var(--border-lighter);
}

.sub-expand {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: center;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2);
  border: none;
  background: transparent;
  color: var(--text-muted);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
}

.sub-expand:hover {
  background: var(--surface-2);
  color: var(--text);
}

.expand-icon {
  display: inline-flex;
  transition: transform 0.18s ease;
}

.expand-icon.is-expanded {
  transform: rotate(180deg);
}

.sub-title-line {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.sub-name {
  min-width: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.sub-url {
  font-size: 12px;
  color: var(--text-faint);
}

/* 展开的线路列表：参考下载页的剧集列表——不用整块底色，
   靠细分隔线 + 行 hover 区分，视觉上更干净 */
.address-box {
  padding: var(--space-3);
  border-top: 1px solid var(--border-lighter);
}

.address-empty {
  margin: 0;
  padding: var(--space-1) 0;
  font-size: 12px;
  color: var(--text-faint);
}

.address-scroll {
  padding-right: var(--space-1);
}

.address-list {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 0;
}

/* 单条线路：无底色、紧凑行距，只有悬停时浮一层浅底 */
.address-item {
  position: relative;
  display: flex;
  align-items: center;
  width: 100%;
  height: auto;
  margin: 0;
  padding: var(--space-3);
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: background-color 0.16s ease;
}

/* 相邻行一根细线分隔（与下载页 download-row + download-row 一致） */
.address-item+.address-item {
  border-top: 1px solid var(--border-lighter);
}

.address-item:hover {
  background: var(--surface-2);
}

/* 不画单选圆圈：选中态直接用行背景表达。
   隐藏的 input 只留 1px，保证键盘 Tab 仍能聚焦；聚焦时给整行一圈描边 */
.address-item :deep(.el-radio__input) {
  position: absolute;
  width: 1px;
  height: 1px;
  opacity: 0;
  pointer-events: none;
}

/* 键盘 Tab 聚焦时才描边（鼠标点选不要出现描边）；:has 不支持时该规则整体忽略 */
.address-item:has(:focus-visible) {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}

/* 当前选中线路 */
.address-item.is-checked {
  background: var(--accent-weak);
}

.address-item.is-checked:hover {
  background: var(--accent-weak-hover);
}

.address-item.is-checked .address-name {
  color: var(--accent);
  font-weight: 600;
}

.address-item :deep(.el-radio__label) {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  padding-left: 0;
}

.address-line {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.address-name {
  min-width: 0;
  font-size: 13px;
  font-weight: 500;
  color: var(--text);
}

.address-url {
  margin-top: 1px;
  font-size: 11.5px;
  color: var(--text-faint);
}

.address-stats {
  margin-top: 2px;
  font-size: 11.5px;
  color: var(--text-muted);
}

.address-stats--muted {
  color: var(--text-faint);
  opacity: 0.8;
}

.address-stats--warn {
  color: var(--warning);
}

.line-warn {
  margin: 0 0 var(--space-2);
  padding: var(--space-1) var(--space-2);
  border-left: 3px solid var(--warning);
  border-radius: var(--radius-sm);
  background: var(--warning-weak);
  color: var(--warning);
  font-size: 12px;
  line-height: 1.5;
}

.enabled-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  margin-top: var(--space-3);
  padding: var(--space-3);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
}

.enabled-info {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.enabled-label {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.enabled-url {
  min-width: 0;
  max-width: 480px;
  font-size: 13px;
  color: var(--text);
}

.enabled-sub {
  min-width: 0;
  max-width: 420px;
  font-size: 12px;
  color: var(--text-faint);
}

.enabled-meta {
  flex-basis: 100%;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
}

.enabled-warn {
  flex-basis: 100%;
  font-size: 12px;
  color: var(--warning);
}

@media (max-width: 860px) {
  .enabled-bar {
    flex-direction: column;
    align-items: stretch;
  }
}
</style>
