<!-- 设置页 · 插件分区：GLSL 着色器包 / lavfi 滤镜包
     插件目录：<app_data_dir>/plugins/<id>/plugin.json
     启用状态、顺序、着色器预设存 <app_data_dir>/plugins.json（后端 PluginStore）。 -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import SolarIcon from '@/components/SolarIcon.vue'
import type { PluginInfo, PluginOverview } from '@/types'

const ui = useUiStore()

const overview = ref<PluginOverview>({ root: '', disabled: false, lastError: '', plugins: [] })
const loading = ref(false)
const busy = ref('')

const hasPlugins = computed(() => overview.value.plugins.length > 0)

/** 插件分两类展示：GLSL 着色器包（glsl-shaders）与 FFmpeg/lavfi 滤镜包（vf/af）。 */
function isShaderPlugin(plugin: PluginInfo): boolean {
  return plugin.shaders.length > 0 || plugin.type === 'shader' || plugin.type === 'bundle'
}

function isFilterPlugin(plugin: PluginInfo): boolean {
  return (
    plugin.videoFilters.length > 0 ||
    plugin.audioFilters.length > 0 ||
    plugin.type === 'filter'
  )
}

interface PluginGroup {
  key: string
  title: string
  hint: string
  plugins: PluginInfo[]
}

function canReorderGroup(group: PluginGroup): boolean {
  return group.plugins.filter((item) => item.enabled).length > 1
}

const pluginGroups = computed<PluginGroup[]>(() => {
  const shader: PluginInfo[] = []
  const filter: PluginInfo[] = []
  const other: PluginInfo[] = []
  for (const plugin of overview.value.plugins) {
    if (isShaderPlugin(plugin)) shader.push(plugin)
    else if (isFilterPlugin(plugin)) filter.push(plugin)
    else other.push(plugin)
  }
  const groups: PluginGroup[] = [
    {
      key: 'shader',
      title: 'GLSL 着色器',
      hint: 'mpv glsl-shaders，处理视频帧',
      plugins: shader,
    },
    {
      key: 'filter',
      title: 'FFmpeg 滤镜',
      hint: 'lavfi vf / af，经 ffmpeg 滤镜链',
      plugins: filter,
    },
  ]
  if (other.length) {
    groups.push({ key: 'other', title: '其它', hint: '未声明着色器或滤镜', plugins: other })
  }
  return groups
})

function typeLabel(plugin: PluginInfo): string {
  if (plugin.type === 'shader') return '着色器'
  if (plugin.type === 'filter') return '滤镜'
  if (plugin.type === 'bundle') return '着色器 + 滤镜'
  return '未声明类型'
}

/** 用途标签，与播放器「画质」浮层的分组保持一致。 */
function purposeLabel(plugin: PluginInfo): string {
  const entries = [...plugin.shaders, ...plugin.videoFilters, ...plugin.audioFilters]
  const category = entries.find((entry) => entry.category)?.category
  switch (category) {
    case 'upscale':
      return '超分'
    case 'hdr':
      return 'HDR / 色彩'
    case 'sharpen':
      return '锐化'
    case 'repair':
      return '修复'
    case 'style':
      return '调色 / 风格'
    default:
      return ''
  }
}

function statusLabel(plugin: PluginInfo): string {
  if (plugin.status === 'error') return '加载失败'
  if (plugin.status === 'invalid') return '格式无效'
  return plugin.enabled ? '已启用' : '未启用'
}

function gpuLabel(load: string): string {
  if (load === 'high') return 'GPU 高'
  if (load === 'low') return 'GPU 低'
  return 'GPU 中'
}

/** 元信息压成一行，避免一排彩色标签喧宾夺主 */
function metaText(plugin: PluginInfo): string {
  const parts = [typeLabel(plugin)]
  if (plugin.version) parts.push(`v${plugin.version}`)
  if (plugin.author) parts.push(`作者 ${plugin.author}`)
  if (plugin.shaders.length) parts.push(`${plugin.shaders.length} 个着色器`)
  if (plugin.videoFilters.length) parts.push(`视频滤镜 ${plugin.videoFilters.length}`)
  if (plugin.audioFilters.length) parts.push(`音频滤镜 ${plugin.audioFilters.length}`)
  const load =
    plugin.shaders[0]?.gpuLoad ||
    plugin.videoFilters[0]?.gpuLoad ||
    plugin.audioFilters[0]?.gpuLoad
  if (load) parts.push(gpuLabel(load))
  return parts.join(' · ')
}

function statusClass(plugin: PluginInfo) {
  if (plugin.status === 'error' || plugin.status === 'invalid') return 'is-error'
  if (plugin.enabled) return 'is-on'
  return ''
}

async function load(rescan = false) {
  if (loading.value) return
  loading.value = true
  try {
    overview.value = rescan ? await api.pluginRescan() : await api.pluginOverview()
  } catch (e) {
    ui.notify('读取插件列表失败：' + api.formatError(e), 'error')
  } finally {
    loading.value = false
  }
}

async function run(key: string, task: () => Promise<PluginOverview>, okText: string) {
  if (busy.value) return
  busy.value = key
  try {
    overview.value = await task()
    ui.notify(okText, 'success')
  } catch (e) {
    ui.notify('操作失败：' + api.formatError(e), 'error')
    await load()
  } finally {
    busy.value = ''
  }
}

function onToggle(plugin: PluginInfo, enabled: boolean) {
  void run(
    `toggle:${plugin.id}`,
    () => api.pluginSetEnabled(plugin.id, enabled),
    enabled ? `已启用「${plugin.name}」` : `已停用「${plugin.name}」`,
  )
}

function onPreset(plugin: PluginInfo, preset: string) {
  void run(
    `preset:${plugin.id}`,
    () => api.pluginSetPreset(plugin.id, preset),
    preset ? '着色器预设已切换' : '已恢复应用全部着色器',
  )
}

/** 分类内上下移动：交换两个插件在全局 order 中的位置，避免跨类跳动。 */
function moveInGroup(group: PluginGroup, plugin: PluginInfo, delta: -1 | 1) {
  const list = group.plugins.filter((item) => item.enabled)
  const index = list.findIndex((item) => item.id === plugin.id)
  const target = index + delta
  if (index < 0 || target < 0 || target >= list.length) return
  const ids = overview.value.plugins.map((item) => item.id)
  const from = ids.indexOf(plugin.id)
  const to = ids.indexOf(list[target].id)
  if (from < 0 || to < 0) return
  ;[ids[from], ids[to]] = [ids[to], ids[from]]
  void run('order', () => api.pluginSetOrder(ids), '插件顺序已保存')
}

function canMoveInGroup(group: PluginGroup, plugin: PluginInfo, delta: -1 | 1): boolean {
  const list = group.plugins.filter((item) => item.enabled)
  const index = list.findIndex((item) => item.id === plugin.id)
  return plugin.enabled && index >= 0 && index + delta >= 0 && index + delta < list.length
}

async function openDir() {
  if (busy.value) return
  busy.value = 'open'
  try {
    await api.pluginOpenDir()
  } catch (e) {
    ui.notify('打开插件目录失败：' + api.formatError(e), 'error')
  } finally {
    busy.value = ''
  }
}

async function openHomepage(plugin: PluginInfo) {
  if (!plugin.homepage) return
  try {
    await api.pluginOpenUrl(plugin.homepage)
  } catch (e) {
    ui.notify('打开插件主页失败：' + api.formatError(e), 'error')
  }
}

const PRESET_OPTIONS = (plugin: PluginInfo) => [
  { label: '全部着色器', value: '' },
  ...plugin.shaders.map((shader) => ({ label: shader.name || shader.id, value: shader.id })),
]

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-plugins">
    <header class="section-head">
      <h2 id="settings-plugins" class="section-title">插件</h2>
    </header>

    <el-alert
      v-if="overview.disabled"
      class="plugin-alert"
      type="warning"
      :closable="false"
      show-icon
      title="插件已被安全模式禁用"
      description="当前以 VBOX_DISABLE_PLUGINS=1 启动，所有插件不会加载；去掉该环境变量后重启应用即可恢复。"
    />

    <div class="field plugin-dir-field">
      <span class="field-label">插件目录</span>
      <div class="field-control plugin-dir-control">
        <el-input
          :model-value="overview.root"
          disabled
          placeholder="读取中…"
          aria-label="插件目录"
        />
        <el-button :disabled="loading || !!busy" @click="openDir">
          <template #icon><SolarIcon name="folder-open" /></template>
          打开目录
        </el-button>
        <el-button :loading="loading" @click="load(true)">
          <template #icon><SolarIcon name="refresh" /></template>
          重新扫描
        </el-button>
      </div>
      <p class="field-hint">
        把含 <code>plugin.json</code> 的插件文件夹放进该目录，再点「重新扫描」。
      </p>
    </div>

    <el-alert
      v-if="overview.lastError"
      class="plugin-alert"
      type="error"
      :closable="false"
      show-icon
      title="最近一次应用插件失败（已回退为空渲染链）"
      :description="overview.lastError"
    />

    <el-empty
      v-if="!hasPlugins && !loading"
      description="还没有插件。把含 plugin.json 的插件文件夹放进上面的目录，再点「重新扫描」。"
      :image-size="72"
    >
      <el-button @click="openDir">
        <template #icon><SolarIcon name="folder-open" /></template>
        打开插件目录
      </el-button>
    </el-empty>

    <div v-else class="plugin-groups">
      <section v-for="group in pluginGroups" :key="group.key" class="plugin-group">
        <div class="plugin-group-head">
          <span class="plugin-group-title">{{ group.title }}</span>
          <span class="plugin-group-hint">{{ group.hint }}</span>
          <span class="plugin-group-count">{{ group.plugins.length }}</span>
        </div>

        <div class="plugin-list-box">
          <el-scrollbar class="plugin-scroll" max-height="300px">
            <ul v-if="group.plugins.length" class="plugin-list">
              <li v-for="plugin in group.plugins" :key="plugin.id" class="plugin-row">
                <div class="plugin-main">
                  <div class="plugin-head">
                    <span class="plugin-name">{{ plugin.name || plugin.id }}</span>
                    <span v-if="plugin.bundled" class="plugin-badge">内置</span>
                    <span v-if="purposeLabel(plugin)" class="plugin-badge">
                      {{ purposeLabel(plugin) }}
                    </span>
                    <button
                      v-if="plugin.homepage"
                      type="button"
                      class="plugin-home"
                      :title="plugin.homepage"
                      @click="openHomepage(plugin)"
                    >
                      <SolarIcon name="square-arrow-right-up" :size="12" />
                      项目地址
                    </button>
                    <span class="plugin-status" :class="statusClass(plugin)">
                      <span class="plugin-status-dot" />
                      {{ statusLabel(plugin) }}
                    </span>
                  </div>
                  <p class="plugin-meta">{{ metaText(plugin) }}</p>
                  <p v-if="plugin.description" class="plugin-desc">{{ plugin.description }}</p>
                  <p v-if="plugin.error" class="plugin-error">{{ plugin.error }}</p>

                  <div v-if="plugin.shaders.length > 1" class="plugin-preset">
                    <span class="plugin-preset-label">着色器预设</span>
                    <el-select
                      :model-value="plugin.preset || ''"
                      class="plugin-preset-select"
                      :disabled="!!busy"
                      aria-label="着色器预设"
                      @change="
                        (value: string | number | boolean | undefined) =>
                          onPreset(plugin, String(value ?? ''))
                      "
                    >
                      <el-option
                        v-for="option in PRESET_OPTIONS(plugin)"
                        :key="option.value"
                        :label="option.label"
                        :value="option.value"
                      />
                    </el-select>
                  </div>
                </div>

                <div class="plugin-side">
                  <el-switch
                    :model-value="plugin.enabled"
                    :disabled="!!busy || plugin.status === 'invalid'"
                    :aria-label="`启用 ${plugin.name}`"
                    @change="
                      (value: string | number | boolean) => onToggle(plugin, Boolean(value))
                    "
                  />
                  <div v-if="canReorderGroup(group) && plugin.enabled" class="plugin-order">
                    <el-button
                      text
                      :disabled="!!busy || !canMoveInGroup(group, plugin, -1)"
                      @click="moveInGroup(group, plugin, -1)"
                    >
                      上移
                    </el-button>
                    <el-button
                      text
                      :disabled="!!busy || !canMoveInGroup(group, plugin, 1)"
                      @click="moveInGroup(group, plugin, 1)"
                    >
                      下移
                    </el-button>
                  </div>
                </div>
              </li>
            </ul>
            <p v-else class="plugin-group-empty">还没有{{ group.title }}插件。</p>
          </el-scrollbar>
        </div>
      </section>
    </div>

    <p class="section-note">
      GLSL 着色器与内置 Anime4K / ArtCNN / FSRCNNX 默认互斥：插件声明
      <code>stackWithBuiltin: true</code> 才会叠加（CAS 与滤镜可叠加）。
      滤镜只允许白名单内的 lavfi 单链，禁止读文件类滤镜（movie / subtitles 等）。
    </p>
  </section>
</template>

<style scoped>
.plugin-alert {
  margin-bottom: var(--space-3);
}

.plugin-dir-field {
  margin-top: 0;
}

.plugin-dir-control :deep(.el-input) {
  flex: 1;
  min-width: 240px;
}

.plugin-groups {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  margin-top: var(--space-3);
}

.plugin-group-head {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  margin-bottom: 6px;
}

.plugin-group-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.plugin-group-hint {
  font-size: 12px;
  color: var(--text-faint);
}

.plugin-group-count {
  margin-left: auto;
  font-size: 12px;
  color: var(--text-faint);
}

.plugin-list-box {
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-inset);
  overflow: hidden;
}

.plugin-group-empty {
  margin: 0;
  padding: var(--space-4);
  text-align: center;
  font-size: 12px;
  color: var(--text-faint);
}

.plugin-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.plugin-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3);
  border-bottom: 1px solid var(--border-lighter);
}

.plugin-row:last-child {
  border-bottom: none;
}

.plugin-main {
  min-width: 0;
  flex: 1;
}

.plugin-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.plugin-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
}

.plugin-badge {
  padding: 3px;
  border-radius: var(--radius-sm);
  background: var(--accent-weak);
  color: var(--accent);
  font-size: 11px;
  line-height: 1;
}

.plugin-status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  color: var(--text-faint);
}

.plugin-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}

.plugin-status.is-on {
  color: var(--success);
}

.plugin-status.is-error {
  color: var(--danger);
}

.plugin-meta {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--text-muted);
  word-break: break-word;
}

.plugin-desc {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--text-muted);
}

.plugin-error {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--danger);
  word-break: break-word;
}

.plugin-home {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 0;
  border: none;
  background: none;
  color: var(--accent);
  font-size: 12px;
  line-height: 1.4;
  cursor: pointer;
}

.plugin-home:hover {
  text-decoration: underline;
}

.plugin-preset {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-top: var(--space-3);
}

.plugin-preset-label {
  flex: none;
  font-size: 12px;
  color: var(--text-muted);
}

.plugin-preset-select {
  width: 320px;
  max-width: 100%;
}

.plugin-side {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: var(--space-2);
  flex: none;
}

.plugin-order {
  display: flex;
  gap: var(--space-1);
}
</style>
