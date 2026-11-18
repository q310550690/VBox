<!-- 云盘 · 账号管理弹窗（W14 §8.5）
     入口：云盘首页右上角按钮。与点播页「选择点播站点」弹窗同款形态（el-dialog + 居中），
     内容复用 W13 的账号面板（AccountPanel），这里只负责弹窗外壳、标题与设置页公共样式容器。 -->
<script setup lang="ts">
import AccountPanel from './AccountPanel.vue'
import '@/views/settings/sections.css'

defineProps<{
  modelValue: boolean
  /** 过期胶囊等入口：打开弹窗后聚焦并高亮该 provider 的卡片（W14 §8.2） */
  focusProvider?: string
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()
</script>

<template>
  <el-dialog
    :model-value="modelValue"
    class="pan-account-dialog"
    title="账号管理"
    width="620px"
    append-to-body
    align-center
    @update:model-value="emit('update:modelValue', $event)"
  >
    <el-scrollbar class="pan-account-scroll">
      <div class="settings-page pan-account-dialog-body">
        <AccountPanel :focus-provider="focusProvider" />
      </div>
    </el-scrollbar>
  </el-dialog>
</template>

<style scoped>
.pan-account-dialog :deep(.el-dialog) {
  display: flex;
  flex-direction: column;
  max-height: 80vh;
}

.pan-account-dialog :deep(.el-dialog__body) {
  flex: 1;
  min-height: 0;
  padding: 4px 16px 16px;
  overflow: hidden;
}

.pan-account-scroll {
  height: 100%;
}

.pan-account-dialog .pan-account-dialog-body :deep(.section-head) {
  /* 弹窗标题已显示「账号管理」，面板内的分区标题不再重复 */
  display: none;
}
</style>