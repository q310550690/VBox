<!-- 云盘 · 账号管理抽屉（W14 §8.5）
     入口：云盘首页右上角设置图标（齿轮）。内容复用 W13 的账号面板（AccountPanel），
     这里只负责抽屉外壳、标题与设置页公共样式的容器。 -->
<script setup lang="ts">
import AccountPanel from './AccountPanel.vue'
import '@/views/settings/sections.css'

defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()
</script>

<template>
  <el-drawer
    :model-value="modelValue"
    title="账号管理"
    size="440px"
    append-to-body
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="settings-page pan-account-drawer">
      <AccountPanel />
    </div>
  </el-drawer>
</template>

<style scoped>
.pan-account-drawer :deep(.section-head) {
  /* 抽屉标题已显示「账号管理」，面板内的分区标题不再重复 */
  display: none;
}
</style>