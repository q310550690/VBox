<!-- 云盘层级布局：/cloud 下的首页 / 搜索结果 / 云盘管理共用。
     父布局被默认布局的 KeepAlive 缓存，从搜索/管理返回首页时不重新加载。 -->
<script setup lang="ts">
import { RouterView } from 'vue-router'
import LayoutFade from '@/components/LayoutFade.vue'

defineOptions({ name: 'CloudLayout' })
</script>

<template>
  <LayoutFade :depth="1">
    <RouterView v-slot="{ Component }">
      <KeepAlive :include="['CloudHomeView', 'CloudSearchView', 'CloudManageView']">
        <component :is="Component" />
      </KeepAlive>
    </RouterView>
  </LayoutFade>
</template>