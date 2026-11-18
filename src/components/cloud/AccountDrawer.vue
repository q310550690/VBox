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

<!-- 弹窗 append-to-body  teleport 到 body，scoped/:deep 选择器无法命中，
     故这里用全局样式约束外壳：弹窗本体限高 80vh 且不整体滚动，
     账号内容交给内部 el-scrollbar 滚动。 -->
<style>
.pan-account-dialog {
  display: flex;
  flex-direction: column;
  max-height: 80vh;
  overflow: hidden;
}

/* 不加自定义 padding：间距沿用 el-dialog 自带 padding。
   高度链路全部走 flex（不用 height:100%）：dialog 80vh 限高 → body flex:1 →
   el-scrollbar 根与内部 wrap 各 flex:1 + min-height:0，wrap 才有确定可视高度并真正滚动。 */
.pan-account-dialog .el-dialog__body {
  flex: 1 1 auto;
  min-height: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.pan-account-dialog .pan-account-scroll {
  flex: 1 1 auto;
  min-height: 0;
  height: auto;
  display: flex;
  flex-direction: column;
}

.pan-account-dialog .pan-account-scroll .el-scrollbar__wrap {
  flex: 1 1 auto;
  min-height: 0;
  height: auto;
}

.pan-account-dialog .pan-account-dialog-body .section-head {
  /* 弹窗标题已显示「账号管理」，面板内的分区标题不再重复 */
  display: none;
}
</style>