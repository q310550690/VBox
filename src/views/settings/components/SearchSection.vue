<!-- 设置页 · 搜索分区：跨源搜索线程数（持久化到 <app_data_dir>/search.json） -->
<script setup lang="ts">
import { onMounted, ref } from 'vue'
import * as api from '@/api'
import { useUiStore } from '@/store/ui'
import type { SearchPrefs } from '@/types'

const ui = useUiStore()

/** 当前设置（后端归一化后回填） */
const prefs = ref<SearchPrefs>({ threads: 8 })
const loading = ref(false)
const saving = ref(false)

/** 线程数选项：与后端 1..=32 的白名单范围一致 */
const THREAD_OPTIONS = [1, 2, 4, 6, 8, 12, 16, 24, 32] as const

async function load() {
  if (loading.value) return
  loading.value = true
  try {
    prefs.value = await api.searchPrefs()
  } catch (e) {
    ui.notify('读取搜索设置失败：' + api.formatError(e), 'error')
  } finally {
    loading.value = false
  }
}

/** 保存整份设置（失败时回滚到后端当前值） */
async function save(patch: Partial<SearchPrefs>) {
  if (saving.value) return
  saving.value = true
  try {
    prefs.value = await api.searchSetPrefs({ ...prefs.value, ...patch })
    ui.notify('搜索设置已保存（下次搜索生效）', 'success')
  } catch (e) {
    ui.notify('保存搜索设置失败：' + api.formatError(e), 'error')
    await load()
  } finally {
    saving.value = false
  }
}

function onThreadsChange(value: string | number | boolean | undefined) {
  const threads = Number(value)
  if (Number.isFinite(threads)) void save({ threads })
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="settings-section" aria-labelledby="settings-search">
    <header class="section-head">
      <h2 id="settings-search" class="section-title">搜索</h2>
      <p class="section-desc">
        搜索页默认全站点搜索：线程数决定同时向多少个站点发起检索，越多越快，但并发过高可能被部分站点限流。
      </p>
    </header>

    <div class="field-grid">
      <div class="field">
        <span class="field-label">搜索线程</span>
        <span class="field-control">
          <el-select
            :model-value="prefs.threads"
            :disabled="loading || saving"
            aria-label="搜索线程数"
            @change="onThreadsChange"
          >
            <el-option
              v-for="threads in THREAD_OPTIONS"
              :key="threads"
              :label="`${threads} 线程`"
              :value="threads"
            />
          </el-select>
        </span>
      </div>
    </div>

    <p class="section-note">
      默认 8 线程（1-32）；站点更少时按站点数并发。
    </p>
  </section>
</template>

<style scoped>
</style>
