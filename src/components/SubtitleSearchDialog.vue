<!-- 在线字幕搜索弹窗：输入片名 → 后端搜索字幕站 → 选定一条下载并自动加载到播放器
     - 搜索/下载都在 Rust 端完成（Tauri 命令 subtitle_sources / subtitle_search / subtitle_download）
     - 浏览器预览没有网络能力：显示提示并禁用搜索
     - 下载成功后 emit('loaded', 本地路径)，由父组件调用 playerAddSubtitle 加载 -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import {
  demoMode,
  formatError,
  subtitleDownload,
  subtitleSearch,
  subtitleSources,
} from '@/api'
import type { OnlineSubtitle, SubtitleSource } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 弹窗显隐（v-model） */
    modelValue: boolean
    /** 打开时的默认关键词（一般传剧名/片名） */
    defaultQuery?: string
  }>(),
  { defaultQuery: '' },
)

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
  (e: 'loaded', path: string): void
}>()

const visible = computed({
  get: () => props.modelValue,
  set: (value: boolean) => emit('update:modelValue', value),
})

/** 在线字幕源（后端下发） */
const sources = ref<SubtitleSource[]>([])
const sourceId = ref('subhd')
const query = ref('')
const results = ref<OnlineSubtitle[]>([])
const loading = ref(false)
/** 是否已经搜过一次（区分「未搜索」与「无结果」） */
const searched = ref(false)
const error = ref('')
const loadingId = ref('')

const sourceHint = computed(
  () => sources.value.find((source) => source.id === sourceId.value)?.hint ?? '',
)

watch(
  () => props.modelValue,
  async (open) => {
    if (!open) return
    if (!query.value.trim() && props.defaultQuery) query.value = props.defaultQuery
    await ensureSources()
    if (query.value.trim()) {
      void doSearch()
    } else {
      results.value = []
      searched.value = false
    }
  },
)

/** 首次打开时拉取可用源（失败不阻塞搜索，展示错误即可） */
async function ensureSources() {
  if (demoMode || sources.value.length) return
  try {
    const list = await subtitleSources()
    if (!list.length) return
    sources.value = list
    if (!list.some((source) => source.id === sourceId.value)) {
      sourceId.value = list[0].id
    }
  } catch (e) {
    error.value = `加载在线字幕源失败：${formatError(e)}`
  }
}

async function doSearch() {
  const keyword = query.value.trim()
  if (!keyword || loading.value || demoMode) return
  loading.value = true
  error.value = ''
  searched.value = true
  try {
    results.value = await subtitleSearch(sourceId.value, keyword)
  } catch (e) {
    results.value = []
    error.value = formatError(e)
  } finally {
    loading.value = false
  }
}

/** 下载一条字幕：成功后关闭弹窗并交给父组件加载 */
async function load(item: OnlineSubtitle) {
  if (loadingId.value) return
  loadingId.value = item.id
  error.value = ''
  try {
    const path = await subtitleDownload(
      item.source || sourceId.value,
      item.id,
      item.name || item.title,
    )
    emit('loaded', path)
    visible.value = false
  } catch (e) {
    error.value = `字幕下载失败：${formatError(e)}`
  } finally {
    loadingId.value = ''
  }
}
</script>

<template>
  <el-dialog
    align-center
    v-model="visible"
    top="8vh"
    append-to-body
    class="subtitle-dialog dlg-lg"
    :close-on-click-modal="true"
  >
    <template #header>
      <div class="dlg-head">
        <SolarIcon name="subtitles" :size="18" />
        <span class="dlg-title">在线字幕</span>
        <span v-if="sourceHint" class="dlg-sub">{{ sourceHint }}</span>
      </div>
    </template>

    <div class="search-row">
      <el-input
        v-model="query"
        class="search-input"
        placeholder="输入片名 / 剧名搜索（如：沙丘）"
        clearable
        :disabled="demoMode"
        @keyup.enter="doSearch"
      >
        <template #prefix>
          <SolarIcon name="magnifier" :size="14" />
        </template>
      </el-input>
      <el-select
        v-model="sourceId"
        class="source-select"
        :disabled="demoMode || sources.length <= 1"
        placeholder="字幕源"
      >
        <el-option
          v-for="source in sources"
          :key="source.id"
          :label="source.name"
          :value="source.id"
        />
      </el-select>
      <el-button
        type="primary"
        :loading="loading"
        :disabled="demoMode || !query.trim()"
        @click="doSearch"
      >
        搜索
      </el-button>
    </div>

    <p v-if="demoMode" class="hint">浏览器预览无法联网搜索字幕，请在桌面端使用。</p>
    <el-alert
      v-else-if="error"
      :title="error"
      type="error"
      show-icon
      :closable="false"
      class="dlg-alert"
    />

    <div v-if="loading" class="state">
      <SolarIcon class="spin" name="refresh" :size="16" />
      <span>正在搜索字幕…</span>
    </div>
    <template v-else>
      <div v-if="results.length" class="result-list">
        <el-scrollbar max-height="46vh">
          <div v-for="item in results" :key="`${item.source}-${item.id}`" class="result-row">
            <div class="result-main">
              <div class="result-title">
                <span class="title-text ellipsis-1" :title="item.title">{{ item.title }}</span>
                <el-tag v-if="item.format" size="small" effect="plain">{{ item.format }}</el-tag>
              </div>
              <div class="result-name ellipsis-1" :title="item.name">{{ item.name }}</div>
              <div class="result-meta">
                <el-tag
                  v-for="lang in item.languages"
                  :key="lang"
                  size="small"
                  type="info"
                  effect="plain"
                >
                  {{ lang }}
                </el-tag>
                <span v-if="item.origin" class="meta-text">{{ item.origin }}</span>
                <span v-if="item.group" class="meta-text">{{ item.group }}</span>
                <span v-if="item.size" class="meta-text">{{ item.size }}</span>
                <span v-if="item.downloads" class="meta-text">{{ item.downloads }} 次下载</span>
                <span v-if="item.date" class="meta-text">{{ item.date }}</span>
                <span v-if="item.uploader" class="meta-text">by {{ item.uploader }}</span>
              </div>
            </div>
            <el-button
              size="small"
              type="primary"
              :loading="loadingId === item.id"
              :disabled="Boolean(loadingId) && loadingId !== item.id"
              @click="load(item)"
            >
              加载
            </el-button>
          </div>
        </el-scrollbar>
      </div>
      <el-empty
        v-else-if="searched && !error"
        description="没有找到字幕，换个关键词试试"
        :image-size="72"
      />
      <p v-else-if="!error" class="hint">
        输入片名后回车搜索；下载完成后会自动加载到当前播放器。
      </p>
    </template>

    <template #footer>
      <span class="dlg-foot">字幕来自各字幕站，版权归原作者所有，仅供语言学习交流。</span>
    </template>
  </el-dialog>
</template>

<style scoped>
.dlg-head {
  display: flex;
  gap: 8px;
  align-items: baseline;
}

.dlg-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
}

.dlg-sub {
  font-size: 11.5px;
  color: var(--text-muted);
}

.search-row {
  display: flex;
  gap: var(--space-2);
  align-items: center;
}

.search-input {
  flex: 1;
  min-width: 0;
}

.source-select {
  width: 130px;
  flex-shrink: 0;
}

.dlg-alert {
  margin-top: 10px;
}

.hint {
  margin: 12px 0 4px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}

.state {
  display: flex;
  gap: 8px;
  align-items: center;
  justify-content: center;
  padding: 28px 0;
  font-size: 12.5px;
  color: var(--text-muted);
}

.spin {
  animation: subtitle-spin 1s linear infinite;
}

@keyframes subtitle-spin {
  to {
    transform: rotate(360deg);
  }
}

.result-list {
  margin-top: 10px;
  border: 0;
  background: var(--surface-inset);
  border-radius: var(--radius-md);
}

.result-row {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border-lighter);
}

.result-row:last-child {
  border-bottom: none;
}

.result-main {
  flex: 1;
  min-width: 0;
}

.result-title {
  display: flex;
  gap: 6px;
  align-items: center;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.title-text {
  min-width: 0;
}

.result-name {
  margin-top: 2px;
  font-size: 12px;
  color: var(--text-regular);
}

.result-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  margin-top: 5px;
}

.meta-text {
  font-size: 11.5px;
  color: var(--text-faint);
}

.dlg-foot {
  font-size: 11px;
  color: var(--text-faint);
}
</style>
