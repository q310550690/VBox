<!-- 设置页 · 外观分区（frontend-design.md §6.8 / §4）：主题三态 + 7 套配色 -->
<script setup lang="ts">
import SolarIcon from '@/components/SolarIcon.vue'
import type { SolarIconName } from '@/icons/solar'
import { THEME_HUES, useUiStore } from '@/store/ui'
import type { ThemeMode } from '@/types'

const ui = useUiStore()

/** 主题三态选项（图标全部来自 src/icons/solar.ts，Solar Linear） */
const MODE_OPTIONS: { value: ThemeMode; label: string; icon: SolarIconName }[] = [
  { value: 'system', label: '跟随系统', icon: 'monitor' },
  { value: 'light', label: '亮色', icon: 'sun' },
  { value: 'dark', label: '暗色', icon: 'moon' },
]

/** el-radio-group 的 change 类型较宽，这里收窄到 ThemeMode */
function onModeChange(val: string | number | boolean | undefined) {
  if (val === 'system' || val === 'light' || val === 'dark') ui.setMode(val)
}

/** 亚克力开关：状态与 <html> 类统一由 ui store 维护（会持久化，重启后恢复） */
function onAcrylicChange(val: string | number | boolean) {
  ui.setAcrylic(Boolean(val))
}

/** 全局背景亚克力（依赖「侧边与标题栏」） */
function onAcrylicContentChange(val: string | number | boolean) {
  ui.setAcrylicContent(Boolean(val))
}

/** 恢复默认外观：主题模式 / 配色 / 亚克力开关 */
function onResetAppearance() {
  ui.resetAppearance()
  ui.notify('已恢复默认外观', 'success')
}
</script>

<template>
  <section class="settings-section panel" aria-labelledby="settings-appearance">
    <header class="section-head">
      <div class="section-head-text">
        <h2 id="settings-appearance" class="section-title">外观</h2>
        <p class="section-desc">主题模式、配色与亚克力效果，修改后立即生效并保存到本地。</p>
      </div>
      <el-button class="reset-btn" @click="onResetAppearance">
        <template #icon>
          <SolarIcon name="restart" />
        </template>
        恢复默认
      </el-button>
    </header>

    <div class="field">
      <span class="field-label">主题模式</span>
      <el-radio-group :model-value="ui.mode" aria-label="主题模式" @change="onModeChange">
        <el-radio v-for="option in MODE_OPTIONS" :key="option.value" :value="option.value">
          <span class="mode-option">
            <SolarIcon :name="option.icon" aria-hidden="true" />
            {{ option.label }}
          </span>
        </el-radio>
      </el-radio-group>
    </div>
    <div class="field">
      <span class="field-label">亚克力效果</span>
      <div class="field-grid">
        <div class="field-control">
          <el-switch :model-value="ui.acrylic" aria-label="侧边与标题栏" @change="onAcrylicChange" />
          <span class="switch-label">侧边与标题栏</span>
        </div>
        <div class="field-control">
          <el-switch :model-value="ui.acrylicContent" aria-label="全局背景" :disabled="!ui.acrylic"
            @change="onAcrylicContentChange" />
          <span class="switch-label">全局背景</span>
        </div>
      </div>
    </div>

    <div class="field">
      <span class="field-label">配色</span>
      <div class="hue-grid" role="radiogroup" aria-label="配色方案">
        <button v-for="hue in THEME_HUES" :key="hue.key" type="button" class="hue-card"
          :class="{ active: ui.hue === hue.key }" role="radio" :aria-checked="ui.hue === hue.key"
          :aria-label="`配色：${hue.label}`" @click="ui.setHue(hue.key)">
          <span class="hue-colors" aria-hidden="true">
            <span v-for="(color, index) in hue.colors" :key="index" class="hue-color"
              :style="{ backgroundColor: color }" />
          </span>
          <span class="hue-label">{{ hue.label }}</span>
          <SolarIcon v-if="ui.hue === hue.key" class="hue-check" name="check-circle" aria-hidden="true" />
        </button>
      </div>
    </div>
    <p class="current-theme">
      当前主题：<strong>{{ ui.hueDef.label }} · {{ ui.modeLabel }}</strong>
      <span class="resolved">
        （实际生效：{{ ui.resolvedMode === 'dark' ? '暗色' : '亮色' }}）
      </span>
    </p>
  </section>
</template>

<style scoped>
.reset-btn {
  flex-shrink: 0;
}


.mode-option {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
}

.hue-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
  gap: var(--space-2);
}

.hue-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text-regular);
  text-align: left;
  cursor: pointer;
  transition: var(--transition-color);
}

.hue-card:hover {
  border-color: var(--accent-hover);
  box-shadow: var(--shadow-1);
}

.hue-card.active {
  border: 2px solid var(--accent);
  padding: calc(var(--space-2) - 1px);
  color: var(--text);
}

.hue-colors {
  display: flex;
  height: 18px;
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.hue-color {
  flex: 1;
}

.hue-label {
  font-size: 12px;
}

.hue-check {
  position: absolute;
  top: var(--space-1);
  right: var(--space-1);
  color: var(--accent);
  background: var(--surface);
  border-radius: var(--radius-pill);
  font-size: 14px;
}

.current-theme {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.current-theme strong {
  color: var(--text);
  font-weight: 600;
}

.resolved {
  color: var(--text-faint);
}
</style>
