<!-- 详情页信息卡：侧栏单列。封面 + 剧名/评分/类型/导演/主演，2×2 操作，
     简介（el-scrollbar 定高滚动）、主演横滑、剧照横滑、影视信息；
     整列高度超出面板时由父级 .info-panel 内的 el-scrollbar 兜底滚动。
     豆瓣数据（主创照片 / 剧照 / 影视信息）来自 douban_detail，失败时只隐藏对应列，不阻塞详情页。 -->
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import * as api from '@/api'
import SolarIcon from '@/components/SolarIcon.vue'
import type { DoubanPerson, DoubanSubjectMeta, VideoDetail } from '@/types'

const props = defineProps<{
  /** 详情数据（接口已就绪） */
  detail: VideoDetail
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

/** 元信息行：年份 · 地区 · 类型（豆瓣 genres 优先，退回列表页分类；不展示来源站点） */
const metaText = computed(() => {
  const parts = [props.detail.year, props.detail.area, genresText.value || props.category]
  return parts.filter((v): v is string => Boolean(v && String(v).trim())).join(' · ')
})

/** 无封面时的首字占位 */
const initial = computed(() => (props.detail.name || '?').trim().charAt(0))

/* ---------------- 豆瓣扩展：主创 / 剧照 / 影视信息 ---------------- */

const people = ref<DoubanPerson[]>([])
const stills = ref<string[]>([])
const meta = ref<DoubanSubjectMeta | null>(null)

/** 请求序号：影片切换 / 快速重选时丢弃过期结果 */
let doubanSeq = 0

/** 按片名去豆瓣找条目；失败静默（隐藏主演列与剧照，影视信息退回详情自带字段） */
async function loadDouban() {
  const name = props.detail?.name?.trim()
  const seq = (doubanSeq += 1)
  if (!name) {
    people.value = []
    stills.value = []
    meta.value = null
    return
  }
  try {
    const data = await api.doubanDetail(name, props.detail?.year ?? undefined)
    if (seq !== doubanSeq) return
    people.value = (data.people ?? []).slice(0, 12)
    stills.value = (data.stills ?? []).slice(0, 4)
    meta.value = data.meta ?? null
  } catch {
    if (seq !== doubanSeq) return
    people.value = []
    stills.value = []
    meta.value = null
  }
}

watch(
  () => [props.detail?.name, props.detail?.year] as const,
  () => {
    void loadDouban()
  },
  { immediate: true },
)

/** 评价人数：12000 → 1.2万 */
function countText(count?: number | null): string {
  if (!count || count <= 0) return ''
  return count >= 10000 ? `${(count / 10000).toFixed(1)}万` : String(count)
}

/** 评价人数文案（评分行右侧） */
const ratingCountText = computed(() => countText(meta.value?.ratingCount))

/** 主演一行：豆瓣前 3 位；无豆瓣数据时退回详情自带的演员文本 */
const actorTop = computed(() => {
  if (people.value.length) return people.value.slice(0, 3).map((person) => person.name).join(' / ')
  return (props.detail.actor ?? '').trim()
})

const genresText = computed(() => (meta.value?.genres ?? []).join(' / '))

/** 影视信息行（底部收尾）：评分 / 地区 / 类型已在头部展示，这里只补剩余字段 */
const facts = computed(() => {
  const m = meta.value
  const rows: Array<{ label: string; value: string }> = [
    { label: '类型', value: genresText.value },
    { label: '上映', value: m?.pubdate ?? '' },
    { label: '片长', value: (m?.durations ?? []).join(' / ') },
    { label: '语言', value: (m?.languages ?? []).join(' / ') },
  ]
  return rows.filter((row) => row.value.trim().length > 0)
})
</script>

<template>
  <section
    class="detail-info"
    aria-label="影片信息"
  >
    <!-- ① 头部：封面 80px + 剧名 / 元信息 / 评分 / 导演 / 主演（侧栏横向压缩，不再四列拼盘） -->
    <div class="hero-head">
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
      <div class="head-info">
        <h1 class="title">{{ detail.name }}</h1>
        <p
          v-if="metaText"
          class="meta"
        >{{ metaText }}</p>
        <div
          v-if="meta?.rating"
          class="rating-line"
          :title="ratingCountText ? `${ratingCountText}人评价` : '豆瓣评分'"
        >
          <SolarIcon
            name="star"
            :size="13"
            class="rating-star"
          />
          <span class="rating-score">{{ meta.rating }}</span>
          <span
            v-if="ratingCountText"
            class="rating-count"
          >{{ ratingCountText }}人评价</span>
        </div>
        <p
          v-if="detail.director"
          class="credits"
        >导演：{{ detail.director }}</p>
        <p
          v-if="actorTop"
          class="credits"
        >主演：{{ actorTop }}</p>
      </div>
    </div>

    <!-- ② 主操作：2×2 等宽（播放 / 收藏 / 下载 / 下载全部） -->
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

    <!-- ③ 简介：定高内 el-scrollbar 滚动（侧栏纵向空间有限，长简介在框内滚动） -->
    <div
      v-if="hasDesc"
      class="desc-block"
    >
      <el-scrollbar
        class="desc-scroll"
        max-height="79px"
      >
        <p class="desc">{{ detail.desc }}</p>
      </el-scrollbar>
    </div>
    <p
      v-else
      class="credits muted desc-empty"
    >暂无简介</p>

    <!-- ④ 主演：横滑头像条（侧栏纵向省空间） -->
    <section
      v-if="people.length"
      class="info-section"
      aria-label="主演"
    >
      <h2 class="col-title">主演</h2>
      <el-scrollbar class="strip-scroll">
        <div class="people-strip">
          <div
            v-for="person in people"
            :key="`${person.name}-${person.role || ''}`"
            class="person"
            :title="person.role ? `${person.name}（${person.role}）` : person.name"
          >
            <img
              v-if="person.avatar"
              :src="person.avatar"
              :alt="person.name"
              loading="lazy"
            />
            <span v-else class="person-fallback">{{ person.name.slice(0, 1) }}</span>
            <span class="person-name">{{ person.name }}</span>
          </div>
        </div>
      </el-scrollbar>
    </section>

    <!-- ⑤ 剧照：16:9 横滑，点击放大 -->
    <section
      v-if="stills.length"
      class="info-section"
      aria-label="剧照"
    >
      <h2 class="col-title">剧照</h2>
      <el-scrollbar class="strip-scroll">
        <div class="stills-strip">
          <el-image
            v-for="(src, index) in stills"
            :key="src"
            class="still"
            :src="src"
            fit="cover"
            lazy
            :preview-src-list="stills"
            :initial-index="index"
            preview-teleported
          />
        </div>
      </el-scrollbar>
    </section>

    <!-- ⑥ 影视信息：豆瓣字段，收在底部（空行不渲染） -->
    <section
      v-if="facts.length"
      class="info-section"
    >
      <h2 class="col-title">影视信息</h2>
      <dl class="facts">
        <template
          v-for="row in facts"
          :key="row.label"
        >
          <dt>{{ row.label }}</dt>
          <dd :title="row.value">{{ row.value }}</dd>
        </template>
      </dl>
    </section>
  </section>
</template>

<style scoped>
.detail-info {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-width: 0;
}

/* ---------------- 头部：封面 + 剧名 / 元信息 ---------------- */

.hero-head {
  display: flex;
  gap: var(--space-3);
  min-width: 0;
}

.poster {
  flex: 0 0 80px;
  width: 80px;
  aspect-ratio: 2 / 3;
  overflow: hidden;
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
  font-size: 28px;
  font-weight: 600;
  color: var(--text-faint);
  background: var(--surface-2);
}

.head-info {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.title {
  display: -webkit-box;
  overflow: hidden;
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  line-height: 1.35;
  color: var(--text);
  letter-spacing: 0.2px;
  overflow-wrap: anywhere;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.meta {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.credits {
  overflow: hidden;
  margin: 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--text-regular);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 评分行：星级 + 分数 + 评价人数（填满封面右侧的留白） */
.rating-line {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  line-height: 1.4;
}

.rating-star {
  color: var(--el-color-warning);
}

.rating-score {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  font-variant-numeric: tabular-nums;
}

.rating-count {
  font-size: 11px;
  color: var(--text-muted);
}

/* ---------------- 2×2 主操作 ---------------- */

.actions {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-2);
}

.actions :deep(.el-button) {
  width: 100%;
  margin-left: 0;
}

/* 禁用按钮不响应鼠标事件，用 span 包裹才能触发 tooltip */
.more-wrap {
  display: flex;
  min-width: 0;
}

/* ---------------- 简介：定高 el-scrollbar ---------------- */

.desc-block {
  min-width: 0;
}

.desc-scroll :deep(.el-scrollbar__view) {
  padding-right: 2px;
}

.desc {
  margin: 0;
  font-size: 12px;
  line-height: 1.65;
  color: var(--text-muted);
  white-space: pre-line;
  overflow-wrap: anywhere;
}

.desc-empty {
  margin: 0;
}

/* ---------------- 分区标题 + 横滑条 ---------------- */

.info-section {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}

.col-title {
  margin: 0;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.5;
  color: var(--text-muted);
  letter-spacing: 0.08em;
}

/* el-scrollbar 横向滚动：view 撑成 max-content，超出部分交给滚动条 */
.strip-scroll :deep(.el-scrollbar__view) {
  width: max-content;
}

.people-strip,
.stills-strip {
  display: flex;
  gap: 8px;
  min-width: 0;
  /* 底部留出横向滚动条的高度（bar 高 6px + 距底 2px），避免滚动条压住头像和集名 */
  padding-bottom: 8px;
}

.person {
  display: flex;
  flex: 0 0 48px;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  width: 48px;
  min-width: 0;
}

.person img,
.person-fallback {
  display: block;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  object-fit: cover;
  background: var(--surface-2);
}

.person-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  color: var(--text-faint);
}

.person-name {
  max-width: 100%;
  overflow: hidden;
  font-size: 11px;
  color: var(--text-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.still {
  flex: 0 0 132px;
  width: 132px;
  aspect-ratio: 16 / 9;
  overflow: hidden;
  border-radius: var(--radius-sm);
  background: var(--surface-2);
  cursor: zoom-in;
}

.still :deep(img) {
  transition: transform var(--dur-base) var(--ease);
}

.still:hover :deep(img) {
  transform: scale(1.04);
}

/* 影视信息（底部）：两列「标签：值」网格，6 条信息 3 行排完，省高度 */
.facts {
  display: grid;
  grid-template-columns: max-content minmax(0, 1fr) max-content minmax(0, 1fr);
  gap: 1px 6px;
  margin: 0;
  font-size: 11px;
  line-height: 1.55;
}

.facts dt {
  color: var(--text-faint);
}

.facts dt::after {
  content: '：';
}

.facts dd {
  margin: 0 8px 0 0;
  color: var(--text-regular);
  overflow-wrap: anywhere;
}
</style>
