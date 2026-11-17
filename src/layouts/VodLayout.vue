<!-- 点播层级布局：/vod 下的首页 / 搜索布局共用。
     父布局被默认布局的 KeepAlive 缓存；搜索布局自己缓存搜索页和详情页。 -->
<script setup lang="ts">
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'

defineOptions({ name: 'VodLayout' })
</script>

<template>
  <LayoutFade :depth="1">
    <RouterView v-slot="{ Component }">
      <KeepAlive :include="['VodView', 'SearchLayout']">
        <component :is="Component" />
      </KeepAlive>
    </RouterView>
  </LayoutFade>
</template>
