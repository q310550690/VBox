<!-- 下载前选择线路（线路通常对应不同清晰度/速度）+ 下载范围（当前集 / 本线路全部） -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import { buildLineLabels } from '@/utils/episodeLines'
import type { EpisodeGroup } from '@/types'

const props = withDefaults(
  defineProps<{
    /** 是否显示 */
    open: boolean
    /** 详情里的全部线路（已过滤空线路） */
    lines: EpisodeGroup[]
    /** 当前播放/选中的线路下标 */
    activeLine: number
    /** 当前集在当前线路内的下标 */
    activeIndex: number
    /** 打开时默认的下载范围 */
    defaultScope?: 'current' | 'all'
    /** 解析/入队中：禁用确认并显示 loading */
    loading?: boolean
  }>(),
  {
    defaultScope: 'current',
    loading: false,
  },
)

const emit = defineEmits<{
  (e: 'update:open', open: boolean): void
  (e: 'confirm', payload: { lineIndex: number; scope: 'current' | 'all' }): void
}>()

const lineIndex = ref(0)
const scope = ref<'current' | 'all'>('current')

watch(
  () => props.open,
  (open) => {
    if (!open) return
    lineIndex.value = Math.min(
      Math.max(props.activeLine, 0),
      Math.max(props.lines.length - 1, 0),
    )
    scope.value = props.defaultScope
  },
  { immediate: true },
)

const currentItems = computed(() => props.lines[lineIndex.value]?.items ?? [])
const currentIndex = computed(() =>
  Math.min(Math.max(props.activeIndex, 0), Math.max(currentItems.value.length - 1, 0)),
)
const lineLabels = computed(() => buildLineLabels(props.lines))

function close() {
  emit('update:open', false)
}

function onConfirm() {
  emit('confirm', { lineIndex: lineIndex.value, scope: scope.value })
}
</script>

<template>
  <el-dialog
    :model-value="open"
    title="选择下载线路"
    width="460px"
    append-to-body
    align-center
    @update:model-value="emit('update:open', $event)"
  >
    <p class="dlg-hint">
      不同线路会保存到「剧名 / 线路」下的独立子目录，互不覆盖；某条线路解析失败时，可换一条线路重试。
    </p>

    <div class="dlg-section">
      <span class="dlg-label">线路</span>
      <el-radio-group v-model="lineIndex" class="line-list" aria-label="下载线路">
        <el-radio
          v-for="(group, index) in lines"
          :key="index"
          :value="index"
          border
          class="line-item"
        >
          {{ lineLabels[index] }} · {{ group.items.length }} 集
        </el-radio>
      </el-radio-group>
    </div>

    <div class="dlg-section">
      <span class="dlg-label">范围</span>
      <el-radio-group v-model="scope" aria-label="下载范围">
        <el-radio value="current">
          当前集（第 {{ currentIndex + 1 }} 集）
        </el-radio>
        <el-radio value="all">
          本线路全部 {{ currentItems.length }} 集
        </el-radio>
      </el-radio-group>
    </div>

    <template #footer>
      <div class="dlg-footer">
        <el-button @click="close">
          <template #icon><SolarIcon name="close" /></template>
          取消
        </el-button>
        <el-button type="primary" :loading="loading" @click="onConfirm">
          <template #icon><SolarIcon name="download" /></template>
          开始下载
        </el-button>
      </div>
    </template>
  </el-dialog>
</template>

<style scoped>
.dlg-hint {
  margin: 0 0 var(--space-4);
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.6;
}

.dlg-section {
  margin-bottom: var(--space-4);
}

.dlg-label {
  display: block;
  margin-bottom: var(--space-2);
  font-size: 13px;
  color: var(--text-regular);
}

.line-list {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.line-item {
  margin-right: 0;
}

.dlg-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
}
</style>
