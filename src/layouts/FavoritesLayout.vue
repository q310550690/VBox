<!-- 收藏层级布局：/favorites 下的收藏列表 / 详情共用。
     详情复用点播的 DetailView；返回时从 KeepAlive 剔除。 -->
<script setup lang="ts">
import { computed } from 'vue'
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'
import { detailCached } from './vodDetailCache'

defineOptions({ name: 'FavoritesLayout' })

const keepAliveInclude = computed(() =>
  detailCached.value
    ? ['FavoritesView', 'DetailView']
    : ['FavoritesView'],
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
