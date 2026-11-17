<!-- 搜索输入框：自动聚焦、Enter 搜索、Esc 清空、前缀搜索图标；搜索按钮为 32px 圆形纯图标 -->
<script setup lang="ts">
import { nextTick, onMounted, ref } from 'vue'
import type { InputInstance } from 'element-plus'
import SolarIcon from '@/components/SolarIcon.vue'

withDefaults(
  defineProps<{
    /** 当前关键词（v-model） */
    modelValue: string
    /** 搜索请求进行中（搜索按钮 loading） */
    loading?: boolean
    /** 占位文案 */
    placeholder?: string
  }>(),
  {
    loading: false,
    placeholder: '输入片名 / 演员 / 关键词',
  },
)

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'search'): void
  (e: 'clear'): void
}>()

const inputRef = ref<InputInstance>()

/** 聚焦输入框（页面空态「换个关键词」与自动聚焦复用） */
function focus() {
  void nextTick(() => inputRef.value?.focus())
}

onMounted(focus)

defineExpose({ focus })

function onUpdate(value: string) {
  emit('update:modelValue', value)
}

/** Enter 触发搜索；Esc 清空输入并通知页面复位结果 */
function onKeydown(evt: Event) {
  const event = evt as KeyboardEvent
  if (event.key === 'Enter') {
    event.preventDefault()
    emit('search')
  } else if (event.key === 'Escape') {
    event.preventDefault()
    emit('update:modelValue', '')
    emit('clear')
  }
}
</script>

<template>
  <div class="search-input">
    <el-input
      ref="inputRef"
      :model-value="modelValue"
      :placeholder="placeholder"
      clearable
      aria-label="搜索关键词"
      @update:model-value="onUpdate"
      @keydown="onKeydown"
    >
      <template #prefix>
        <SolarIcon name="magnifier" size="1em" />
      </template>
    </el-input>
    <el-tooltip content="搜索" placement="bottom">
      <el-button
        type="primary"
        circle
        :loading="loading"
        aria-label="搜索"
        @click="emit('search')"
      >
        <template #icon><SolarIcon name="magnifier" size="1em" /></template>
      </el-button>
    </el-tooltip>
  </div>
</template>

<style scoped>
.search-input {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  min-width: 0;
}

.search-input :deep(.el-input) {
  flex: 1;
  min-width: 0;
}
</style>
