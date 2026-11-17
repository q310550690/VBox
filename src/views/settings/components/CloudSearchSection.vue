<!-- 设置页 · 云盘搜索分区：内置内核状态 + PanSou 可选增强源配置（W14 P1） -->
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import * as api from '@/api'
import type { CloudPansouEnv, CloudSourceStatus } from '@/api/cloud'
import { useUiStore } from '@/store/ui'

const ui = useUiStore()

/** 搜索源状态（内置内核 + PanSou） */
const statuses = ref<CloudSourceStatus[]>([])
const loading = ref(false)
const saving = ref(false)
const testing = ref(false)

/** PanSou 表单（留空 = 未配置 / 关闭） */
const pansouUrl = ref('')
const pansouToken = ref('')

/** 远端频道清单（高级，默认关闭）：清单地址 + RSA 公钥 PEM */
const sourcesUrl = ref('')
const sourcesPubkey = ref('')
const savingSources = ref(false)
const updatingManifest = ref(false)

/** PanSou Docker 运行环境（W14 §8.7） */
const pansouEnv = ref<CloudPansouEnv | null>(null)
const detectingEnv = ref(false)

async function loadEnv() {
  if (detectingEnv.value) return
  detectingEnv.value = true
  try {
    pansouEnv.value = await api.cloudPansouEnv()
  } catch (e) {
    ui.notify('检测 Docker 环境失败：' + api.formatError(e), 'error')
  } finally {
    detectingEnv.value = false
  }
}

function openInstall() {
  const url = pansouEnv.value?.installUrl || 'https://docs.docker.com/desktop/'
  api.pluginOpenUrl(url)
}

const builtin = computed(() => statuses.value.find((item) => item.kind === 'builtin_kernel'))
const pansou = computed(() => statuses.value.find((item) => item.kind === 'pan_sou'))

async function load() {
  if (loading.value) return
  loading.value = true
  try {
    statuses.value = await api.cloudSourceStatus()
  } catch (e) {
    ui.notify('读取云盘搜索源状态失败：' + api.formatError(e), 'error')
  } finally {
    loading.value = false
  }
}

async function save() {
  if (saving.value) return
  saving.value = true
  try {
    statuses.value = await api.cloudSourceSet({
      kind: 'pansou',
      url: pansouUrl.value.trim() || null,
      token: pansouToken.value.trim() || null,
    })
    ui.notify(
      pansouUrl.value.trim() ? 'PanSou 增强源已保存' : '已关闭 PanSou 增强源',
      'success',
    )
    pansouUrl.value = ''
    pansouToken.value = ''
  } catch (e) {
    ui.notify('保存云盘搜索源失败：' + api.formatError(e), 'error')
  } finally {
    saving.value = false
  }
}

async function test() {
  if (testing.value) return
  testing.value = true
  try {
    const message = await api.cloudSourceTest()
    ui.notify(message, 'success')
  } catch (e) {
    ui.notify('搜索源自检失败：' + api.formatError(e), 'error')
  } finally {
    testing.value = false
  }
}

async function saveSources() {
  if (savingSources.value) return
  savingSources.value = true
  try {
    statuses.value = await api.cloudSourcesConfigSet({
      sourcesUrl: sourcesUrl.value.trim(),
      sourcesPubkey: sourcesPubkey.value.trim(),
    })
    ui.notify('频道清单配置已保存', 'success')
  } catch (e) {
    ui.notify('保存频道清单配置失败：' + api.formatError(e), 'error')
  } finally {
    savingSources.value = false
  }
}

async function updateManifest() {
  if (updatingManifest.value) return
  updatingManifest.value = true
  try {
    const typed = sourcesUrl.value.trim()
    const message = await api.cloudSourcesUpdate(typed || undefined)
    ui.notify(message, 'success')
    await load()
  } catch (e) {
    ui.notify('更新频道清单失败：' + api.formatError(e), 'error')
  } finally {
    updatingManifest.value = false
  }
}

function disablePansou() {
  pansouUrl.value = ''
  pansouToken.value = ''
  void save()
}

onMounted(() => {
  void load()
  void loadEnv()
})
</script>

<template>
  <section
    class="settings-section"
    aria-labelledby="settings-cloudsearch"
  >
    <header class="section-head">
      <h2
        id="settings-cloudsearch"
        class="section-title"
      >
        云盘搜索源
      </h2>
      <p class="section-desc">
        云盘聚合搜索默认使用内置 Telegram 频道内核，零配置可用；内置源依赖 Telegram 网络连通性（部分网络环境需要代理）。
        PanSou 是可选增强源，需自行部署（Docker 等），VBox 不随包分发。
      </p>
    </header>

    <div class="cloud-source-row">
      <div class="cloud-source-info">
        <span class="field-label">内置内核</span>
        <span class="cloud-source-detail">
          {{ builtin?.detail || '加载中…' }}
          <template v-if="builtin?.version"> · 清单 {{ builtin.version }}</template>
        </span>
      </div>
      <el-tag
        :type="builtin?.ready ? 'success' : 'info'"
        size="small"
        effect="plain"
      >
        {{ builtin?.ready ? '可用' : '不可用' }}
      </el-tag>
    </div>

    <div class="cloud-source-row">
      <div class="cloud-source-info">
        <span class="field-label">PanSou 增强源</span>
        <span class="cloud-source-detail">
          {{ pansou?.detail || '未配置（可选增强源，默认关闭）' }}
        </span>
      </div>
      <el-tag
        :type="pansou?.ready ? 'success' : 'info'"
        size="small"
        effect="plain"
      >
        {{ pansou?.ready ? '已连接' : '未启用' }}
      </el-tag>
    </div>

    <div class="field-grid">
      <div class="field">
        <span class="field-label">服务地址</span>
        <span class="field-control">
          <el-input
            v-model="pansouUrl"
            placeholder="http://127.0.0.1:8888（留空表示关闭）"
            :disabled="loading || saving"
            clearable
            aria-label="PanSou 服务地址"
          />
        </span>
      </div>
      <div class="field">
        <span class="field-label">服务 token</span>
        <span class="field-control">
          <el-input
            v-model="pansouToken"
            placeholder="可选；Authorization: Bearer"
            :disabled="loading || saving"
            show-password
            clearable
            aria-label="PanSou 服务 token"
          />
        </span>
      </div>
    </div>

    <div class="cloud-source-actions">
      <el-button
        type="primary"
        :loading="saving"
        @click="save"
      >
        保存
      </el-button>
      <el-button
        :loading="testing"
        @click="test"
      >
        测试连接
      </el-button>
      <el-button
        v-if="pansou?.ready"
        :disabled="saving"
        @click="disablePansou"
      >
        关闭增强源
      </el-button>
    </div>

    <div
      v-if="pansouEnv"
      class="cloud-manifest"
    >
      <div class="cloud-manifest-head">
        <span class="field-label">Docker 运行环境</span>
        <span class="cloud-source-detail">
          PanSou 官方目前仅提供 Docker 镜像，无三平台单文件发行版；VBox 不随包分发、不自动启动容器。
        </span>
      </div>
      <div class="cloud-source-row">
        <div class="cloud-source-info">
          <span class="cloud-source-detail">
            <template v-if="!pansouEnv.dockerInstalled">未检测到 Docker</template>
            <template v-else>
              Docker {{ pansouEnv.dockerVersion || '已安装' }} ·
              {{ pansouEnv.daemonRunning ? '守护进程运行中' : '守护进程未运行' }} ·
              镜像
              {{
                pansouEnv.imagePresent
                  ? '已就绪'
                  : '未拉取'
              }}
              <template v-if="pansouEnv.updateAvailable"> · 有新版本</template>
            </template>
          </span>
          <span class="cloud-source-detail">{{ pansouEnv.hint }}</span>
          <span
            v-if="pansouEnv.imageDigest"
            class="cloud-source-detail"
          >
            本地镜像 {{ pansouEnv.imageDigest.slice(0, 19) }}…
          </span>
        </div>
        <el-tag
          :type="pansouEnv.daemonRunning && pansouEnv.imagePresent ? 'success' : 'info'"
          size="small"
          effect="plain"
        >
          {{ pansouEnv.daemonRunning && pansouEnv.imagePresent ? '就绪' : '待配置' }}
        </el-tag>
      </div>
      <div class="cloud-source-actions">
        <el-button
          :loading="detectingEnv"
          @click="loadEnv"
        >
          重新检测
        </el-button>
        <el-button
          v-if="!pansouEnv.dockerInstalled"
          @click="openInstall"
        >
          安装说明
        </el-button>
        <el-button
          v-else-if="!pansouEnv.imagePresent"
          @click="api.pluginOpenUrl(pansouEnv.upstream)"
        >
          打开上游
        </el-button>
      </div>
      <p class="cloud-source-note">
        镜像 <code>{{ pansouEnv.image }}</code>；更新方式：<code>docker pull</code> 后重启容器。
        远程 PanSou 服务由服务端自行维护，客户端只做连通性检测。
      </p>
    </div>

    <div class="cloud-manifest">
      <div class="cloud-manifest-head">
        <span class="field-label">频道清单（高级）</span>
        <span class="cloud-source-detail">
          内置 Telegram 频道可能失效；可配置远端签名清单热更新（默认关闭，校验失败自动回退内置清单）。
        </span>
      </div>
      <div class="field-grid">
        <div class="field">
          <span class="field-label">清单地址</span>
          <span class="field-control">
            <el-input
              v-model="sourcesUrl"
              placeholder="https://example.com/sources.json（留空表示不使用）"
              :disabled="loading || savingSources"
              clearable
              aria-label="远端频道清单地址"
            />
          </span>
        </div>
        <div class="field">
          <span class="field-label">RSA 公钥（PEM）</span>
          <span class="field-control">
            <el-input
              v-model="sourcesPubkey"
              type="textarea"
              :rows="3"
              placeholder="-----BEGIN PUBLIC KEY-----"
              :disabled="loading || savingSources"
              aria-label="远端频道清单公钥"
            />
          </span>
        </div>
      </div>
      <div class="cloud-source-actions">
        <el-button
          type="primary"
          :loading="savingSources"
          @click="saveSources"
        >
          保存清单配置
        </el-button>
        <el-button
          :loading="updatingManifest"
          @click="updateManifest"
        >
          立即更新频道清单
        </el-button>
      </div>
      <p class="cloud-source-note">
        清单需由维护者用 RSA 私钥签名（PKCS#1 v1.5 + SHA-256，签名载荷为移除
        <code>signature</code> 后的紧凑 JSON）；客户端只保存验签通过的原文。
      </p>
    </div>

    <p class="cloud-source-note">
      隐私说明：搜索关键词只在请求时发往所选搜索源，不写入日志；服务地址与 token 仅保存在本机
      <code>cloud-sources.json</code>（不是网盘凭据）。
    </p>
  </section>
</template>

<style scoped>
.cloud-source-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--border-lighter);
}

.cloud-source-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.cloud-source-detail {
  font-size: 13px;
  color: var(--text-secondary);
}

.field-grid {
  margin-top: var(--space-4);
}

.cloud-source-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: var(--space-4);
}

.cloud-source-note {
  margin: var(--space-4) 0 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-tertiary);
}

.cloud-manifest {
  margin-top: var(--space-5);
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-lighter);
}

.cloud-manifest-head {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-bottom: var(--space-2);
}

.cloud-source-note code {
  padding: 0 4px;
  border-radius: 4px;
  background: var(--surface-2);
  font-size: 12px;
}
</style>