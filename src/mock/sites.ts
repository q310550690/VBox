// 点播能力演示数据（mock）
// 用途：后端未就绪 / 浏览器预览（无 Tauri IPC）时，页面仍可完整演示四态与交互。
// 约定：mock 数据只用在一个地方——src/api/index.ts 门面；页面直接 import 门面。
// 禁止把演示数据伪装成真实能力：页面会在页头标注「演示数据」。
import type {
  AddressGroup,
  AddResult,
  Category,
  Episode,
  EpisodeGroup,
  PlayResult,
  Site,
  Subscription,
  TvConfig,
  VideoDetail,
  VideoItem,
} from '@/types'

/* ---------------- 演示站点 ---------------- */

export const DEMO_SITES: Site[] = [
  {
    key: 'demo_cms_cuiwei',
    name: '演示 · 翠微影视',
    type: 1,
    api: 'http://demo.local/api.php/provide/vod/',
    searchable: true,
    playable: true,
  },
  {
    key: 'demo_cms_shuangyue',
    name: '演示 · 霜月剧场',
    type: 1,
    api: 'http://demo.local/shuangyue/api.php/provide/vod/',
    searchable: true,
    playable: true,
  },
  {
    key: 'demo_cms_yanshan',
    name: '演示 · 烟山放映室',
    type: 1,
    api: 'http://demo.local/yanshan/api.php/provide/vod/',
    searchable: true,
    playable: true,
  },
  { key: 'demo_drpy', name: '演示 · Spider 源', type: 3, api: 'http://demo.local/drpy.js' },
  { key: 'demo_xml', name: '演示 · XML 源', type: 0, api: 'http://demo.local/xml.php' },
  {
    key: 'demo_cms_old',
    name: '演示 · 已停更站点',
    type: 5,
    api: 'http://demo.local/old/api.php/provide/vod/',
  },
]

const DEMO_SITE_KEYS = new Set(DEMO_SITES.map((s) => s.key))

/* ---------------- 演示订阅 / 地址 ---------------- */

export const DEMO_SUBSCRIPTIONS: Subscription[] = [
  { name: '演示订阅 · 听墨合集（多仓）', url: 'http://demo.local/dc.txt' },
]

export const DEMO_WAREHOUSE_ADDRESSES = [
  { name: '演示 · 翠微仓', url: 'http://demo.local/wh/cuiwei.json' },
  { name: '演示 · 霜月仓', url: 'http://demo.local/wh/shuangyue.json' },
  { name: '演示 · 失效仓', url: 'http://demo.local/wh/broken.json' },
]

export const DEMO_ENABLED = DEMO_WAREHOUSE_ADDRESSES[0].url

export function mockAddressGroup(url: string): AddressGroup {
  if (url === DEMO_SUBSCRIPTIONS[0].url) {
    return { kind: 'multi', addresses: DEMO_WAREHOUSE_ADDRESSES }
  }
  return { kind: 'single', addresses: [{ name: '演示单仓', url }] }
}

export const DEMO_CONFIG: TvConfig = {
  sites: DEMO_SITES,
  lives: [{ name: '演示直播源', type: 0, url: 'http://demo.local/live.m3u' }],
  parses: [],
  urls: [],
  spider: null,
}

export function mockAddSubscription(name: string, url: string): AddResult {
  const exists = DEMO_SUBSCRIPTIONS.some((s) => s.url === url)
  const subscriptions = exists
    ? DEMO_SUBSCRIPTIONS
    : [...DEMO_SUBSCRIPTIONS, { name, url }]
  return { subscriptions, enabled: DEMO_ENABLED, config: DEMO_CONFIG }
}

export function mockRemoveSubscription(url: string): Subscription[] {
  return DEMO_SUBSCRIPTIONS.filter((s) => s.url !== url)
}

/* ---------------- 演示分类 / 列表 / 详情 ---------------- */

export const DEMO_CATEGORIES: Category[] = [
  { type_id: '1', type_name: '电影' },
  { type_id: '2', type_name: '剧集' },
  { type_id: '3', type_name: '综艺' },
  { type_id: '4', type_name: '动漫' },
  { type_id: '5', type_name: '纪录片' },
  { type_id: '6', type_name: '短剧' },
]

const TITLES = [
  '长夜将明',
  '山海之间',
  '雾城疑云',
  '流浪星球',
  '盛夏回声',
  '无声告白',
  '风起长安',
  '深海纪元',
  '人生海海',
  '星轨之外',
  '雪落长街',
  '幻境棋局',
  '末日余晖',
  '春日列车',
  '归途有风',
  '千面之城',
  '月下孤影',
  '时间的形状',
  '荒野来信',
  '第七个黎明',
  '南方以南',
  '燃烧的平原',
  '蓝调时刻',
  '静止的河流',
  '未完待续',
  '光年之外',
  '逆光而行',
  '白昼流星',
  '海岛书店',
  '最后一班地铁',
  '隐秘的角落',
  '风与海的女儿',
  '赛博旧梦',
  '北纬三十度',
  '琥珀',
  '沉默的证人',
  '昨日青空',
  '一厘米的宇宙',
  '萤火森林',
  '钢铁之雨',
]

const REMARKS = ['HD中字', '4K · 更新至 20 集', '抢先版', '全 40 集', '蓝光原盘', '已完结']

function makeId(catId: string, page: number, index: number) {
  return `demo-${catId}-${page}-${index}`
}

/** 生成某分类某页的演示片单（每页 18 条；第 4 页起为空，用于演示"已到最后一页"）
 *  category 为空表示"全部"，演示数据回退到第一个分类（后端 type=1 源"全部"由接口决定）。 */
export function mockListVideos(site: string, category: string, page: number): VideoItem[] {
  if (!DEMO_SITE_KEYS.has(site)) return []
  const catId = category || DEMO_CATEGORIES[0].type_id
  if (page > 3) return []
  const offset = (page - 1) * 18
  return Array.from({ length: 18 }, (_, i) => {
    const n = offset + i
    const catName = DEMO_CATEGORIES.find((c) => c.type_id === catId)?.type_name ?? '影视'
    return {
      id: makeId(catId, page, i),
      name: `${TITLES[n % TITLES.length]}${n >= TITLES.length ? ' 第' + (Math.floor(n / TITLES.length) + 1) + '季' : ''}`,
      cover: `https://picsum.photos/seed/vbox-${catId}-${n}/300/450`,
      remark: REMARKS[n % REMARKS.length] + ' · ' + catName,
    }
  })
}

/** 分类变更时清空演示片单里的"备注"无意义，保留真实接口语义 */
export function mockListCategories(site: string): Category[] {
  return DEMO_SITE_KEYS.has(site) ? DEMO_CATEGORIES : []
}

export function mockGetDetail(_site: string, id: string): VideoDetail {
  const m = /^demo-(\d+)-(\d+)-(\d+)$/.exec(id)
  const catId = m?.[1] ?? '1'
  const page = Number(m?.[2] ?? 1)
  const index = Number(m?.[3] ?? 0)
  const name = TITLES[(page - 1) * 18 + index] ?? '演示影片'
  const cover = `https://picsum.photos/seed/vbox-${catId}-${(page - 1) * 18 + index}/300/450`
  const groups: EpisodeGroup[] = [
    { group: '演示线路 A（直链）', items: makeEpisodes(id, 0, 20) },
    { group: '演示线路 B（需解析）', items: makeEpisodes(id, 1, 20) },
  ]
  return {
    id,
    name,
    cover,
    year: '2026',
    area: '中国大陆',
    director: '演示导演',
    actor: '演示演员甲 / 演示演员乙 / 演示演员丙',
    desc:
      '这是一条演示简介：VBox 当前为前端阶段，点播内容来自 mock 适配层。' +
      '所有真实能力（列表、详情、直链判定）在 Tauri 桌面端接入后端命令后即会替换此处数据；' +
      '你可以切换顶部源 tab 体验不同站点的状态，或在设置页管理订阅源。',
    episodes: groups,
  }
}

function makeEpisodes(id: string, line: number, count: number): Episode[] {
  return Array.from({ length: count }, (_, i) => ({
    title: `第${i + 1}集`,
    url:
      line === 0
        ? `http://demo.local/media/${encodeURIComponent(id)}/ep${i + 1}.mp4`
        : `http://demo.local/parse/${encodeURIComponent(id)}?ep=${i + 1}`,
  }))
}

export function mockResolve(url: string): PlayResult {
  if (!url) throw new Error('解析失败：空地址')
  if (/\.(mp4|m3u8|flv|mkv)(\?|$)/i.test(url) && url.includes('demo.local')) {
    return { url, headers: {}, subtitles: [] }
  }
  throw new Error('演示数据：该集不是直链，需要解析链（后端未实现）')
}
