<!-- 详情页右侧信息面板（frontend-design.md §6.2）：封面 + 标题/元信息/主创/简介 + 主操作
     桌面端嵌入播放器布局时使用：内容列方向排列，高度受限时由外层 aside 滚动 -->
<script setup lang="ts">
import { computed, ref } from 'vue'
import SolarIcon from '@/components/SolarIcon.vue'
import type { VideoDetail } from '@/types'

const props = defineProps<{
  /** 详情数据（接口已就绪） */
  detail: VideoDetail
  /** 当前站点名（详情数据本身不含来源信息，用于补充元信息） */
  siteName?: string
  /** 进入详情时的分类名（来自列表页路由 state，接口无该字段） */
  category?: string
  /** 「下载」按钮解析中 */
  downloadLoading?: boolean
  /** 「下载全部」按钮解析/入队中 */
  downloadAllLoading?: boolean
  /** 无选集时禁用下载按钮 */
  downloadDisabled?: boolean
  /** 当前影片是否已收藏 */
  favorited?: boolean
  /** 收藏操作进行中 */
  favoriteLoading?: boolean
}>()

const emit = defineEmits<{
  (e: 'play'): void
  (e: 'download'): void
  (e: 'download-all'): void
  (e: 'toggle-favorite'): void
}>()

/** 封面加载失败时回退为首字占位（禁止出现破图） */
const coverFailed = ref(false)

const hasDesc = computed(() => Boolean(props.detail.desc && props.detail.desc.trim()))

/** 元信息行：年份 · 地区 · 分类（VideoDetail 契约中无分类字段，分类来自列表页路由 state；缺省退回来源站点） */
const metaText = computed(() => {
  const parts = [props.detail.year, props.detail.area, props.category || props.siteName]
  return parts.filter((v): v is string => Boolean(v && String(v).trim())).join(' · ')
})

/** 无封面时的首字占位 */
const initial = computed(() => (props.detail.name || '?').trim().charAt(0))
</script>

<template>
  <section
    class="detail-info"
    aria-label="影片信息"
  >
    <div class="hero-head">
      <!-- 封面：严格 2:3，懒加载，失败降级为首字块 -->
      <div class="poster">
        <img
          v-if="detail.cover && !coverFailed"
          :src="detail.cover"
          :alt="detail.name"
          loading="lazy"
          @error="coverFailed = true"
        />
        <div
          v-else
          class="poster-fallback"
          aria-hidden="true"
        >{{ initial }}</div>
      </div>

      <div class="headline">
        <h1 class="title">{{ detail.name }}</h1>
        <p
          v-if="metaText"
          class="meta"
        >{{ metaText }}</p>
        <div v-if="detail.director">
          <p class="credits ellipsis-2">导演：{{ detail.director }}</p>
        </div>

        <div class="actions">
          <el-button
            type="primary"
            @click="emit('play')"
          >
            <template #icon>
              <SolarIcon
                name="play"
                :size="14"
              />
            </template>
            播放
          </el-button>
          <el-tooltip
            :content="favorited ? '取消收藏' : '收藏这部影片'"
            placement="top"
          >
            <el-button
              :type="favorited ? 'warning' : undefined"
              :plain="favorited"
              :loading="favoriteLoading"
              :aria-pressed="favorited"
              @click="emit('toggle-favorite')"
            >
              <template #icon>
                <SolarIcon
                  name="star"
                  :size="14"
                />
              </template>
              {{ favorited ? '已收藏' : '收藏' }}
            </el-button>
          </el-tooltip>
        </div>
        <div class="actions">
          <el-tooltip
            :content="downloadDisabled ? '暂无可下载的选集' : '下载当前集（解析直链后加入下载队列）'"
            placement="top"
          >
            <span class="more-wrap">
              <el-button
                :loading="downloadLoading"
                :disabled="downloadDisabled || downloadAllLoading"
                aria-label="下载当前集"
                @click="emit('download')"
              >
                <template #icon>
                  <SolarIcon
                    name="download"
                    :size="14"
                  />
                </template>下载
              </el-button>
            </span>
          </el-tooltip>
          <el-tooltip
            :content="downloadDisabled ? '暂无可下载的选集' : '解析当前线路全部选集并加入下载队列'"
            placement="top"
          >
            <span class="more-wrap">
              <el-button
                :loading="downloadAllLoading"
                :disabled="downloadDisabled || downloadLoading"
                aria-label="下载全部选集"
                @click="emit('download-all')"
              >
                <template #icon>
                  <SolarIcon
                    name="download-minimalistic"
                    :size="14"
                  />
                </template>下载全部
              </el-button>
            </span>
          </el-tooltip>
        </div>
      </div>
    </div>

    <!-- 演员：最多 2 行，超出省略 -->
    <div v-if="detail.actor">
      <p class="credits ellipsis-2">演员：{{ detail.actor }}</p>
    </div>

    <!-- 简介：占满信息面板剩余高度，超出部分在内部滚动 -->
    <div
      v-if="hasDesc"
      class="desc-block"
    >
      <el-scrollbar class="desc-scroll">
        <p class="desc">{{ detail.desc }}</p>
      </el-scrollbar>
    </div>
    <p
      v-else
      class="credits muted"
    >暂无简介</p>
  </section>
</template>

<style scoped>
.detail-info {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
  height: 100%;
  min-height: 0;
  overflow: hidden;
}

.hero-head {
  display: flex;
  align-items: flex-start;
  gap: var(--space-3);
  min-width: 0;
}

.poster {
  flex: 0 0 104px;
  width: 104px;
  aspect-ratio: 2 / 3;
  overflow: hidden;
  border: 0;
  border-radius: var(--radius-md);
  background: var(--surface-2);
}

.poster img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.poster-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  font-size: 34px;
  font-weight: 600;
  color: var(--text-faint);
  background: var(--surface-2);
}

.headline {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}

.title {
  margin: 0;
  font-size: 19px;
  font-weight: 600;
  line-height: 1.35;
  color: var(--text);
  letter-spacing: 0.2px;
  overflow-wrap: anywhere;
}

.meta {
  margin: 0;
  font-size: 12.5px;
  color: var(--text-muted);
}

.credits {
  margin: 0;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--text-regular);
}

.desc-block {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
  min-width: 0;
  overflow: hidden;
}

/* 滚动条容器撑满剩余高度（el-scrollbar 需要确定高度才会滚动） */
.desc-block :deep(.el-scrollbar) {
  flex: 1;
  min-height: 0;
  height: 100%;
}

/* 详情页右列始终与播放器等高（6:4 不换行）：窄窗口时压缩的是简介可视区域，不改成单列 */

.desc {
  margin: 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--text-muted);
  white-space: pre-line;
  overflow-wrap: anywhere;
}

.actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-1);
}

/* 禁用按钮不响应鼠标事件，用 span 包裹才能触发 tooltip */
.more-wrap {
  display: inline-flex;
}

@media (max-width: 720px) {
  .poster {
    flex-basis: 88px;
    width: 88px;
  }

  .title {
    font-size: 17px;
  }
}
</style>
