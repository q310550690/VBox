<!-- 点播搜索层级布局：/vod/search 下的搜索页 / 详情页共用。
     详情页返回时由 evictDetailView() 暂时从 include 中移除，确保页面销毁；
     侧栏切走时不调用，详情页仍随本布局缓存。 -->
<script setup lang="ts">
import { computed } from 'vue'
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'
import { detailCached } from './vodDetailCache'

defineOptions({ name: 'SearchLayout' })

const keepAliveInclude = computed(() =>
  detailCached.value
    ? ['SearchView', 'DetailView']
    : ['SearchView'],
)
</script>

<template>
  <LayoutFade :depth="2">
    <RouterView v-slot="{ Component }">
      <KeepAlive :include="keepAliveInclude">
        <component :is="Component" />
      </KeepAlive>
    </RouterView>
  </LayoutFade>
</template>
