// 收藏演示数据（仅浏览器预览使用）：localStorage 持久化，语义与 Rust SQLite 后端对齐
// （按 siteKey + vodId 唯一，列表按 createdAt 倒序）
import type { FavoriteItem } from '@/types'

const STORAGE_KEY = 'vbox.favorites'

function read(): FavoriteItem[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
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
