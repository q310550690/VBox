// 应用入口：Vue 3 + Pinia + Vue Router + Element Plus
// 图标统一使用 src/assets/solar-linear/ 的 Solar Linear（经 components/SolarIcon.vue 渲染）；
// 颜色/字体全部由 styles/theme.scss（听墨主题）提供
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ElementPlus from 'element-plus'
import zhCn from 'element-plus/es/locale/lang/zh-cn'

// 样式加载顺序不可调换：
// 1) Element Plus 基础样式  2) 官方暗色变量  3) 听墨多主题  4) 设计 token / 全局基础
// 5) Element Plus 用户覆盖层（输入框/下拉框等自定义样式，必须最后加载）
import 'element-plus/dist/index.css'
import 'element-plus/theme-chalk/dark/css-vars.css'
import './styles/theme.scss'
import './styles/tokens.css'
import './styles/base.css'
import './styles/element-user.scss'

import { getCurrentWindow } from '@tauri-apps/api/window'

import App from './App.vue'
import router from './router'

const app = createApp(App)
app.use(createPinia())
app.use(ElementPlus, { locale: zhCn })
app.use(router)
app.mount('#app')

// 窗口创建时即显示（tauri.conf.json 的 visible: true），由 index.html 里的内联启动占位
// 保证「立刻有画面」；挂载完成后的首帧把占位淡出。浏览器预览同样适用。
const bootSplash = document.getElementById('boot-splash')
if (bootSplash) {
  const dismissSplash = () => {
    bootSplash.classList.add('boot-splash--hide')
    window.setTimeout(() => bootSplash.remove(), 220)
  }
  requestAnimationFrame(() => requestAnimationFrame(dismissSplash))
}

// 兜底：若窗口仍以隐藏方式创建（自定义配置/旧二进制），挂载后主动 show。
if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
  let shown = false
  const reveal = () => {
    if (shown) return
    shown = true
    const win = getCurrentWindow()
    void win.show().then(() => win.setFocus()).catch((err) => console.warn('[vbox] 窗口显示失败', err))
  }
  requestAnimationFrame(() => requestAnimationFrame(reveal))
  window.setTimeout(reveal, 3000)
}
