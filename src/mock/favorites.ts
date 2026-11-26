// 收藏演示数据（仅浏览器预览使用）：localStorage 持久化，语义与 Rust SQLite 后端对齐
// （按 siteKey + vodId 唯一，列表按 createdAt 倒序）
import type { FavoriteItem } from '@/types'

const STORAGE_KEY = 'vbox.favorites.v2'
/** 首次进入（无本地收藏）时的演示收藏，便于浏览器预览看到列表 */
const DEMO_FAVORITES: FavoriteItem[] = [
  { siteKey: 'demo_cms_cuiwei', siteName: '演示 · 翠微影视', vodId: 'demo-1-1-0-0', name: '长夜将明', cover: 'https://picsum.photos/seed/vbox-1-0/300/450', year: '2026', area: '中国大陆', remark: 'HD中字', createdAt: Date.now() - 1000 * 60 * 30 },
  { siteKey: 'demo_cms_shuangyue', siteName: '演示 · 霜月剧场', vodId: 'demo-2-1-3-3', name: '雾城疑云', cover: 'https://picsum.photos/seed/vbox-2-3/300/450', year: '2025', area: '中国大陆', remark: '全 40 集', createdAt: Date.now() - 1000 * 60 * 60 * 3 },
  { siteKey: 'demo_cms_qinghe', siteName: '演示 · 青禾影库', vodId: 'demo-1-2-1-25', name: '山海之间', cover: 'https://picsum.photos/seed/vbox-1-25/300/450', year: '2024', area: '中国香港', remark: '4K · 更新至 20 集', createdAt: Date.now() - 1000 * 60 * 60 * 26 },
  { siteKey: 'demo_cms_lanxi', siteName: '演示 · 澜溪剧场', vodId: 'demo-3-2-5-41', name: '赛博旧梦', cover: 'https://picsum.photos/seed/vbox-3-41/300/450', year: '2023', area: '日本', remark: '蓝光原盘', createdAt: Date.now() - 1000 * 60 * 60 * 24 * 4 },
  { siteKey: 'demo_cms_haijiao', siteName: '演示 · 海角影院', vodId: 'demo-4-2-2-38', name: '灵笼', cover: 'https://picsum.photos/seed/vbox-4-38/300/450', year: '2022', area: '中国大陆', remark: '已完结', createdAt: Date.now() - 1000 * 60 * 60 * 24 * 9 },
  { siteKey: 'demo_cms_xingchen', siteName: '演示 · 星宸影视', vodId: 'demo-1-3-4-52', name: '长安三万里', cover: 'https://picsum.photos/seed/vbox-1-52/300/450', year: '2025', area: '中国大陆', remark: 'HDR · 杜比视界', createdAt: Date.now() - 1000 * 60 * 60 * 24 * 15 },
  { siteKey: 'demo_cms_yanshan', siteName: '演示 · 烟山放映室', vodId: 'demo-2-2-8-50', name: '深海纪元', cover: 'https://picsum.photos/seed/vbox-2-50/300/450', year: '2021', area: '美国', remark: 'TC 抢先', createdAt: Date.now() - 1000 * 60 * 60 * 24 * 22 },
  { siteKey: 'demo_cms_old', siteName: '演示 · 已停更站点', vodId: 'demo-1-1-7-7', name: '旧日回声', cover: 'https://picsum.photos/seed/vbox-1-7/300/450', year: '2019', area: '英国', remark: '已停更', createdAt: Date.now() - 1000 * 60 * 60 * 24 * 40 },
]


function read(): FavoriteItem[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) {
      write(DEMO_FAVORITES)
      return DEMO_FAVORITES
    }
    const data = JSON.parse(raw)
    return Array.isArray(data) ? (data as FavoriteItem[]) : []
  } catch {
    return []
  }
}

function write(items: FavoriteItem[]): FavoriteItem[] {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items))
  } catch {
    /* 存储不可用时忽略 */
  }
  return items
}

/** 读取全部收藏（createdAt 倒序） */
export function loadFavorites(): FavoriteItem[] {
  return [...read()].sort((a, b) => (b.createdAt || 0) - (a.createdAt || 0))
}

/** 新增/更新收藏（同站点同影片覆盖），返回最新列表 */
export function addFavorite(item: FavoriteItem): FavoriteItem[] {
  const others = read().filter(
    (it) => !(it.siteKey === item.siteKey && it.vodId === item.vodId),
  )
  return write(
    [{ ...item, createdAt: item.createdAt || Date.now() }, ...others].sort(
      (a, b) => (b.createdAt || 0) - (a.createdAt || 0),
    ),
  )
}

/** 取消收藏，返回最新列表 */
export function removeFavorite(siteKey: string, vodId: string): FavoriteItem[] {
  return write(read().filter((it) => !(it.siteKey === siteKey && it.vodId === vodId)))
}

/** 单条收藏状态 */
export function isFavorite(siteKey: string, vodId: string): boolean {
  return read().some((it) => it.siteKey === siteKey && it.vodId === vodId)
}
