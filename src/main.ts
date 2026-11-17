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

import App from './App.vue'
import router from './router'

const app = createApp(App)
app.use(createPinia())
app.use(ElementPlus, { locale: zhCn })
app.use(router)
app.mount('#app')
