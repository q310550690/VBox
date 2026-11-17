<!-- 嵌套 Layout 内的页面过渡：只用 CSS 动画，避免 Transition 组件与 KeepAlive
     一起移动缓存 DOM 时触发 leave 回调错乱（insertBefore / leaveCbKey / parentNode 报错）。
     depth 对应 route.matched 中本 Layout 的直接子路由层级：
     VodLayout=1、SearchLayout=2，历史/收藏/直播 Layout=1。 -->
<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { useRoute } from 'vue-router'

const props = defineProps<{ depth: number }>()

const route = useRoute()
const entering = ref(false)
let timer: number | null = null

watch(
  () => route.matched[props.depth]?.path,
  () => {
    entering.value = false
    window.requestAnimationFrame(() => {
      entering.value = true
      if (timer !== null) window.clearTimeout(timer)
      timer = window.setTimeout(() => {
        entering.value = false
        timer = null
      }, 200)
    })
  },
)

onBeforeUnmount(() => {
  if (timer !== null) window.clearTimeout(timer)
})
</script>

<template>
  <div class="layout-fade" :class="{ 'is-entering': entering }">
    <slot />
  </div>
</template>

<style scoped>
.layout-fade {
  height: 100%;
}

.layout-fade.is-entering {
  animation: layout-fade-in 0.16s ease;
}

@keyframes layout-fade-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}
</style>
