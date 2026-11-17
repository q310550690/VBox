<!-- 直播层级布局：/live 下的频道列表 / 频道详情共用。
     父布局被默认布局的 KeepAlive 缓存，从详情返回列表时不重新加载分组。 -->
<script setup lang="ts">
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'

defineOptions({ name: 'LiveLayout' })
</script>

<template>
  <LayoutFade :depth="1">
    <RouterView v-slot="{ Component }">
      <KeepAlive :include="['LiveView', 'LiveDetailView']">
        <component :is="Component" />
      </KeepAlive>
    </RouterView>
  </LayoutFade>
</template>
