<!-- 历史记录层级布局：/history 下的历史列表 / 详情共用。
     详情复用点播的 DetailView；返回时从 KeepAlive 剔除。 -->
<script setup lang="ts">
import { computed } from 'vue'
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'
import { detailCached } from './vodDetailCache'

defineOptions({ name: 'HistoryLayout' })

const keepAliveInclude = computed(() =>
  detailCached.value
    ? ['HistoryView', 'DetailView']
    : ['HistoryView'],
)
</script>

<template>
  <LayoutFade :depth="1">
    <RouterView v-slot="{ Component }">
      <KeepAlive :include="keepAliveInclude">
        <component :is="Component" />
      </KeepAlive>
    </RouterView>
  </LayoutFade>
</template>
