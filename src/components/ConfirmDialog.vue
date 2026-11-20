<!-- 确认对话框：清空历史 / 删除下载等破坏性操作的二次确认（frontend-design.md §6.6、§6.7） -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'

const props = defineProps<{
  /** 是否显示对话框 */
  open: boolean
  /** 标题 */
  title: string
  /** 正文说明 */
  message?: string
  /** 确认按钮文案，默认「确定」 */
  confirmText?: string
  /** 取消按钮文案，默认「取消」 */
  cancelText?: string
  /** 危险操作：确认按钮使用 danger 样式 */
  danger?: boolean
  /** 可选复选框文案（如「同时删除本地文件」）；不传则不显示 */
  checkboxLabel?: string
  /** 复选框当前是否勾选（配合 update:checkbox 使用） */
  checkbox?: boolean
}>()

const emit = defineEmits<{
  (e: 'confirm'): void
  (e: 'cancel'): void
  (e: 'update:open', open: boolean): void
  (e: 'update:checkbox', checked: boolean): void
}>()

/**
 * 主动关闭（点确认/取消）后，el-dialog 在离场动画结束时会再派发一次
 * update:model-value(false)；用该标记避免重复触发 cancel。
 */
const selfClosing = ref(false)

/** 重新打开时复位标记，保证下一次关闭仍能正确派发 cancel */
watch(
  () => props.open,
  (visible) => {
    if (visible) selfClosing.value = false
  },
)

const resolvedConfirmText = computed(() => props.confirmText || '确定')
const resolvedCancelText = computed(() => props.cancelText || '取消')

/** 确认：先派发 confirm，再由组件请求父组件关闭 */
function handleConfirm() {
  selfClosing.value = true
  emit('confirm')
  emit('update:open', false)
}

/** 取消按钮 */
function handleCancel() {
  selfClosing.value = true
  emit('cancel')
  emit('update:open', false)
}

/** el-dialog 被用户关闭（右上角 × / 遮罩 / Esc）时也会走这里，按「取消」处理 */
function handleVisibleChange(visible: boolean) {
  if (visible) {
    emit('update:open', true)
    return
  }
  if (!selfClosing.value) emit('cancel')
  selfClosing.value = false
  emit('update:open', false)
}
</script>

<template>
  <el-dialog
    :model-value="open"
    :title="title"
    class="dlg-sm"
    append-to-body
    align-center
    @update:model-value="handleVisibleChange"
  >
    <p v-if="message" class="confirm-message" :class="{ 'is-danger': danger }">{{ message }}</p>

    <div v-if="checkboxLabel" class="confirm-checkbox">
      <el-checkbox
        :model-value="checkbox === true"
        @update:model-value="emit('update:checkbox', $event === true)"
      >
        {{ checkboxLabel }}
      </el-checkbox>
    </div>

    <template #footer>
      <div class="confirm-footer">
        <el-button @click="handleCancel">{{ resolvedCancelText }}</el-button>
        <el-button
          :type="danger ? 'danger' : 'primary'"
          :class="{ 'is-solid': danger }"
          @click="handleConfirm"
        >
          {{ resolvedConfirmText }}
        </el-button>
      </div>
    </template>
  </el-dialog>
</template>

<style scoped>
.confirm-message {
  margin: 0;
  color: var(--text-regular);
  font-size: 14px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

/* 危险确认：正文独立成一条淡红提示条，与"取消"拉开安全距离 */
.confirm-message.is-danger {
  padding: 10px 12px;
  border: 0;
  border-left: 3px solid var(--el-color-danger);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--el-color-danger) 10%, transparent);
  color: var(--el-text-color-primary);
}

.confirm-checkbox {
  margin-top: var(--space-3);
}

.confirm-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2);
}
</style>
