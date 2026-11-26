// 应用路由（frontend-design.md §3）：侧栏 5 项 + 点播/直播/历史/收藏层级子页
// Tauri 文件路径下 hash 路由更稳，保持 createWebHashHistory。
//
// 路由层级：
//   /vod                          点播（父布局）
//     ├─ /vod                     点播首页
//     └─ /vod/search（SearchLayout）
//        ├─ /vod/search          点播搜索
//        └─ /vod/search/detail/... 点播详情
//   /live                         直播（父布局）
//     ├─ /live                    直播频道列表
//     └─ /live/detail/...         直播频道详情
//   /history（HistoryLayout）
//     ├─ /history                 历史记录
//     └─ /history/detail/...      历史详情（复用 DetailView）
//   /favorites（FavoritesLayout）
//     ├─ /favorites               收藏
//     └─ /favorites/detail/...    收藏详情（复用 DetailView）
//   /downloads /settings /local/:id 保持顶级单页
import { createRouter, createWebHashHistory } from 'vue-router'
import type { RouteRecordRaw } from 'vue-router'

import { rememberNavRoute } from './navMemory'

import VodLayout from '@/layouts/VodLayout.vue'
import LiveLayout from '@/layouts/LiveLayout.vue'
import CloudLayout from '@/layouts/CloudLayout.vue'
import SearchLayout from '@/layouts/SearchLayout.vue'
import HistoryLayout from '@/layouts/HistoryLayout.vue'
import FavoritesLayout from '@/layouts/FavoritesLayout.vue'
import VodView from '@/views/vod/index.vue'
import CloudHomeView from '@/views/cloud/index.vue'
import CloudSearchView from '@/views/cloud/search.vue'
import CloudManageView from '@/views/cloud/manage.vue'
import DetailView from '@/views/detail/index.vue'
import LocalPlayerView from '@/views/local-player/index.vue'
import SearchView from '@/views/search/index.vue'
import LiveView from '@/views/live/index.vue'
import LiveDetailView from '@/views/live/detail.vue'
import FavoritesView from '@/views/favorites/index.vue'
import HistoryView from '@/views/history/index.vue'
import DownloadsView from '@/views/downloads/index.vue'
import LogWindowView from '@/views/log-window/index.vue'
import PlayerWindowView from '@/views/player-window/index.vue'
import SettingsView from '@/views/settings/index.vue'

const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/vod' },
  {
    path: '/vod',
    component: VodLayout,
    meta: { nav: 'vod' },
    children: [
      {
        path: '',
        name: 'vod',
        component: VodView,
        meta: { title: '点播', nav: 'vod' },
      },
      {
        path: 'search',
        component: SearchLayout,
        meta: { title: '搜索', nav: 'vod' },
        children: [
          {
            path: '',
            name: 'vod-search',
            component: SearchView,
            meta: { title: '搜索', nav: 'vod', back: '/vod' },
          },
          {
            path: 'detail/:site/:id',
            name: 'vod-detail',
            component: DetailView,
            meta: { title: '详情', nav: 'vod', back: '/vod/search' },
          },
        ],
      },
    ],
  },
  {
    path: '/cloud',
    component: CloudLayout,
    meta: { nav: 'cloud' },
    children: [
      {
        path: '',
        name: 'cloud',
        component: CloudHomeView,
        meta: { title: '云盘', nav: 'cloud' },
      },
      {
        path: 'search/:keyword',
        name: 'cloud-search',
        component: CloudSearchView,
        meta: { title: '云盘搜索', nav: 'cloud', back: '/cloud' },
      },
      {
        path: 'manage/:accountId',
        name: 'cloud-manage',
        component: CloudManageView,
        meta: { title: '云盘管理', nav: 'cloud', back: '/cloud' },
      },
    ],
  },
  {
    path: '/live',
    component: LiveLayout,
    meta: { nav: 'live' },
    children: [
      {
        path: '',
        name: 'live',
        component: LiveView,
        meta: { title: '直播', nav: 'live' },
      },
      {
        path: 'detail/:group/:channel',
        name: 'live-detail',
        component: LiveDetailView,
        meta: { title: '直播详情', nav: 'live', back: '/live' },
      },
    ],
  },
  {
    path: '/history',
    component: HistoryLayout,
    meta: { nav: 'history' },
    children: [
      {
        path: '',
        name: 'history',
        component: HistoryView,
        meta: { title: '历史记录', nav: 'history' },
      },
      {
        path: 'detail/:site/:id',
        name: 'history-detail',
        component: DetailView,
        meta: { title: '详情', nav: 'history', back: '/history' },
      },
    ],
  },
  {
    path: '/favorites',
    component: FavoritesLayout,
    meta: { nav: 'favorites' },
    children: [
      {
        path: '',
        name: 'favorites',
        component: FavoritesView,
        meta: { title: '收藏', nav: 'favorites' },
      },
      {
        path: 'detail/:site/:id',
        name: 'favorites-detail',
        component: DetailView,
        meta: { title: '详情', nav: 'favorites', back: '/favorites' },
      },
    ],
  },
  {
    path: '/local/:id',
    name: 'local-player',
    component: LocalPlayerView,
    meta: { title: '本地播放', nav: 'downloads', back: '/downloads' },
  },
  {
    path: '/downloads',
    name: 'downloads',
    component: DownloadsView,
    meta: { title: '下载', nav: 'downloads' },
  },
  {
    path: '/player-window',
    name: 'player-window',
    component: PlayerWindowView,
    meta: { title: '播放器' },
  },
  {
    path: '/settings',
    name: 'settings',
    component: SettingsView,
    meta: { title: '设置', nav: 'settings' },
  },
  // 独立运行日志窗口（由 Rust 打开，不经过主布局）
  {
    path: '/log-window',
    name: 'log-window',
    component: LogWindowView,
    meta: { title: '运行日志', nav: 'settings' },
  },
  { path: '/:pathMatch(.*)*', redirect: '/vod' },
]

const router = createRouter({
  history: createWebHashHistory(),
  routes,
  // 页面滚动实际发生在内容区的 el-scrollbar 里（不是 window），这条只约束 window：
  // 每页独立的滚动位置由 layouts/default 按路由 fullPath 记忆/恢复。
  scrollBehavior: () => ({ top: 0 }),
})

// 离开详情页时先让后端暂停播放：KeepAlive 会缓存详情页，组件的
// onDeactivated / 路由监听在极端时序下可能晚于布局 DOM 移动；
// 这里在导航确认前直接调 player_pause 兜底，避免切到设置/其它菜单后声音还在继续。
const DETAIL_ROUTE_NAMES = new Set(['vod-detail', 'history-detail', 'favorites-detail'])
router.beforeEach((to, from) => {
  if (!DETAIL_ROUTE_NAMES.has(String(from.name ?? ''))) return
  // 详情页之间互相切换时由新详情页直接接管播放，异步 pause 可能误伤新播放器。
  if (DETAIL_ROUTE_NAMES.has(String(to.name ?? ''))) return
  void import('@/api')
    .then((api) => {
      if (api.isTauri) void api.playerPause().catch(() => {})
    })
    .catch(() => {})
})

// 每个一级菜单记住最后停留的页面，供侧栏点击时恢复。
router.afterEach((to) => {
  rememberNavRoute(to)
})

export default router
