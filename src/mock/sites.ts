// 点播能力演示数据（mock，2026-11-25 扩充）
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
// 16 个站点：覆盖苹果CMS（type1）、Spider（type3）、XML（type0）、停更（type5），
// 首页站点 tab 更多，列表/详情/搜索各处都能看到不同数据。

export const DEMO_SITES: Site[] = [
  { key: 'demo_cms_cuiwei', name: '演示 · 翠微影视', type: 1, api: 'http://demo.local/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_shuangyue', name: '演示 · 霜月剧场', type: 1, api: 'http://demo.local/shuangyue/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_yanshan', name: '演示 · 烟山放映室', type: 1, api: 'http://demo.local/yanshan/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_qinghe', name: '演示 · 青禾影库', type: 1, api: 'http://demo.local/qinghe/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_lanxi', name: '演示 · 澜溪剧场', type: 1, api: 'http://demo.local/lanxi/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_muxi', name: '演示 · 暮汐资源', type: 1, api: 'http://demo.local/muxi/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_haijiao', name: '演示 · 海角影院', type: 1, api: 'http://demo.local/haijiao/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_tianji', name: '演示 · 天际线', type: 1, api: 'http://demo.local/tianji/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_xingchen', name: '演示 · 星宸影视', type: 1, api: 'http://demo.local/xingchen/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_guyu', name: '演示 · 谷雨片单', type: 1, api: 'http://demo.local/guyu/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_cangshan', name: '演示 · 苍山点播', type: 1, api: 'http://demo.local/cangshan/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_cms_nanyu', name: '演示 · 南屿资源', type: 1, api: 'http://demo.local/nanyu/api.php/provide/vod/', searchable: true, playable: true },
  { key: 'demo_drpy', name: '演示 · Spider 源', type: 3, api: 'http://demo.local/drpy.js' },
  { key: 'demo_drpy2', name: '演示 · Spider 源 2', type: 3, api: 'http://demo.local/drpy2.js' },
  { key: 'demo_xml', name: '演示 · XML 源', type: 0, api: 'http://demo.local/xml.php' },
  { key: 'demo_cms_old', name: '演示 · 已停更站点', type: 5, api: 'http://demo.local/old/api.php/provide/vod/' },
]

const DEMO_SITE_KEYS = new Set(DEMO_SITES.map((s) => s.key))

/* ---------------- 演示订阅 / 地址 ---------------- */

export const DEMO_SUBSCRIPTIONS: Subscription[] = [
  { name: '演示订阅 · 听墨合集（多仓）', url: 'http://demo.local/dc.txt' },
  { name: '演示订阅 · 星河片库', url: 'http://demo.local/xinghe.txt' },
  { name: '演示订阅 · 聚合仓（含失效线路）', url: 'http://demo.local/juhe.txt' },
]

export const DEMO_WAREHOUSE_ADDRESSES = [
  { name: '演示 · 翠微仓', url: 'http://demo.local/wh/cuiwei.json' },
  { name: '演示 · 霜月仓', url: 'http://demo.local/wh/shuangyue.json' },
  { name: '演示 · 星河仓', url: 'http://demo.local/wh/xinghe.json' },
  { name: '演示 · 聚合仓', url: 'http://demo.local/wh/juhe.json' },
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
  lives: [
    { name: '演示直播源 · IPTV', type: 0, url: 'http://demo.local/live.m3u' },
    { name: '演示直播源 · 备用', type: 0, url: 'http://demo.local/live-backup.m3u' },
    { name: '演示直播源 · Spider', type: 3, api: 'http://demo.local/live.js' },
  ],
  parses: [
    { name: '演示解析 · 线路 A', type: 1, url: 'http://demo.local/jx/a' },
    { name: '演示解析 · 线路 B', type: 1, url: 'http://demo.local/jx/b' },
    { name: '演示解析 · 超解析', type: 4, url: 'http://demo.local/jx/super' },
    { name: '演示解析 · 嗅探', type: 0, url: 'http://demo.local/jx/sniff' },
  ],
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
  { type_id: '7', type_name: '动作片' },
  { type_id: '8', type_name: '喜剧片' },
  { type_id: '9', type_name: '爱情片' },
  { type_id: '10', type_name: '科幻片' },
]

const TITLES = [
  '长夜将明', '山海之间', '雾城疑云', '流浪星球', '盛夏回声', '无声告白',
  '风起长安', '深海纪元', '人生海海', '星轨之外', '雪落长街', '幻境棋局',
  '末日余晖', '春日列车', '归途有风', '千面之城', '月下孤影', '时间的形状',
  '荒野来信', '第七个黎明', '南方以南', '燃烧的平原', '蓝调时刻', '静止的河流',
  '未完待续', '光年之外', '逆光而行', '白昼流星', '海岛书店', '最后一班地铁',
  '隐秘的角落', '风与海的女儿', '赛博旧梦', '北纬三十度', '琥珀', '沉默的证人',
  '昨日青空', '一厘米的宇宙', '萤火森林', '钢铁之雨', '白日焰火', '无名之辈',
  '暴雪将至', '银河补习班', '悬崖之上', '万里归途', '孤勇者', '追风者',
  '长安三万里', '雄狮少年', '哪吒重生', '姜子牙', '白蛇缘起', '大护法',
  '大鱼海棠', '肆式青春', '罗小黑战记', '刺客伍六七', '灵笼', '雾山五行',
  '时光代理人', '一人之下', '狐妖小红娘', '镇魂街', '全职高手', '斗破苍穹',
  '完美世界', '遮天', '一念永恒', '星辰变', '武动乾坤', '沧元图',
  '吞噬星空', '凡人修仙传', '少年歌行', '眷思量', '枕刀歌', '万国志',
  '中国奇谭', '深海', '茶啊二中', '熊出没', '喜羊羊与灰太狼', '猪猪侠',
  '超级飞侠', '海底小纵队', '名侦探柯南', '哆啦A梦', '灌篮高手', '足球小将',
  '排球少年', '咒术回战', '鬼灭之刃', '进击的巨人', '间谍过家家', '葬送的芙莉莲',
  '孤独摇滚', '电锯人', '辉夜大小姐想让我告白', '我的青春恋爱物语果然有问题', '冰菓', '紫罗兰永恒花园',
]

const REMARKS = [
  'HD中字', '4K · 更新至 20 集', '抢先版', '全 40 集', '蓝光原盘', '已完结',
  'HDR · 杜比视界', '更新至 12 集', 'TC 抢先', '花絮合集',
]

const DIRECTORS = [
  '张晓波', '陈凯歌', '娄烨', '贾樟柯', '刁亦男', '文牧野', '饶晓志', '曹保平',
  '宁浩', '徐峥', '陈思诚', '郭帆', '田晓鹏', '饺子', '程耳', '管虎',
]

const ACTORS = [
  '肖战', '王一博', '张译', '秦海璐', '周迅', '黄渤', '易烊千玺', '刘浩存',
  '雷佳音', '杨幂', '朱一龙', '倪妮', '张子枫', '刘昊然', '王传君', '章宇',
  '沈腾', '马丽', '贾玲', '张小斐', '吴京', '段奕宏', '廖凡', '桂纶镁',
  '胡歌', '唐嫣', '刘亦菲', '陈晓', '杨紫', '李现', '赵丽颖', '林更新',
]

const AREAS = ['中国大陆', '中国香港', '中国台湾', '日本', '韩国', '美国', '英国', '法国', '泰国', '西班牙']

const DESCS = [
  '一段关于选择与告别的故事。当命运的岔路突然出现，几个原本毫无交集的人被推到一起，他们必须在有限的时间里做出决定，也重新认识了自己。',
  '影片以细腻的镜头语言描摹普通人的日常，在琐碎的生活缝隙里，藏着最动人的情感与最坚韧的力量。',
  '当真相被层层迷雾包裹，追查者一步步逼近核心，却发现最难以面对的，是自己心底那个不愿承认的答案。',
  '一场跨越十年的约定，把散落各地的人重新聚拢。旧照片、老唱片和那条熟悉的长街，都在提醒他们：有些东西从未改变。',
  '在城市的边缘，一群普通人用自己的方式守护着彼此。他们的故事没有惊天动地，却足以照亮漫长黑夜。',
  '改编自真实事件，讲述一群年轻人在时代浪潮中逆流而上的十年。友情、理想与代价，在大银幕上缓缓展开。',
]

const PER_PAGE = 24
const MAX_PAGE = 6

/** 站点哈希：让不同站点在同一分类下看到不同片单 */
function siteOffset(site: string): number {
  let h = 0
  for (let i = 0; i < site.length; i += 1) h = (h * 31 + site.charCodeAt(i)) % 9973
  return h % 97
}

function titleAt(n: number): string {
  const base = TITLES[n % TITLES.length]
  const season = Math.floor(n / TITLES.length)
  return season > 0 ? `${base} 第${season + 1}季` : base
}

function coverAt(catId: string, n: number): string {
  return `https://picsum.photos/seed/vbox-${catId}-${n}/300/450`
}

/** 生成某分类某页的演示片单（每页 24 条；共 6 页，第 7 页起为空，用于演示「已到最后一页」）
 *  category 为空表示「全部」，演示数据回退到第一个分类。 */
export function mockListVideos(site: string, category: string, page: number): VideoItem[] {
  if (!DEMO_SITE_KEYS.has(site)) return []
  const catId = category || DEMO_CATEGORIES[0].type_id
  if (page > MAX_PAGE) return []
  const catIndex = DEMO_CATEGORIES.findIndex((c) => c.type_id === catId)
  const catName = DEMO_CATEGORIES.find((c) => c.type_id === catId)?.type_name ?? '影视'
  const base = (page - 1) * PER_PAGE + siteOffset(site) + Math.max(0, catIndex) * 5
  return Array.from({ length: PER_PAGE }, (_, i) => {
    const n = base + i
    return {
      id: `demo-${catId}-${page}-${i}-${n}`,
      name: titleAt(n),
      cover: coverAt(catId, n),
      remark: `${REMARKS[n % REMARKS.length]} · ${catName}`,
    }
  })
}

/** 分类变更时清空演示片单里的「备注」无意义，保留真实接口语义 */
export function mockListCategories(site: string): Category[] {
  return DEMO_SITE_KEYS.has(site) ? DEMO_CATEGORIES : []
}

export function mockGetDetail(_site: string, id: string): VideoDetail {
  const m = /^demo-(\d+)-(\d+)-(\d+)(?:-(\d+))?$/.exec(id)
  const catId = m?.[1] ?? '1'
  const page = Number(m?.[2] ?? 1)
  const index = Number(m?.[3] ?? 0)
  const n = m?.[4] !== undefined ? Number(m[4]) : (page - 1) * PER_PAGE + index
  const name = titleAt(n)
  const actors = Array.from({ length: 6 }, (_, k) => ACTORS[(n + k * 7) % ACTORS.length]).join(' / ')
  const groups: EpisodeGroup[] = [
    { group: '线路一 · 高清直链', items: makeEpisodes(id, 1, 24) },
    { group: '线路二 · 云播备用', items: makeEpisodes(id, 2, 24) },
    { group: '线路三 · 解析线路', items: makeEpisodes(id, 3, 12) },
  ]
  return {
    id,
    name,
    cover: coverAt(catId, n),
    year: String(2015 + (n % 12)),
    area: AREAS[n % AREAS.length],
    director: DIRECTORS[n % DIRECTORS.length],
    actor: actors,
    desc: `${DESCS[n % DESCS.length]}${DESCS[(n + 2) % DESCS.length]}${DESCS[(n + 4) % DESCS.length]}`,
    episodes: groups,
  }
}

function makeEpisodes(id: string, line: number, count: number): Episode[] {
  return Array.from({ length: count }, (_, i) => ({
    title: `第${i + 1}集`,
    url:
      line === 1
        ? `http://demo.local/media/${encodeURIComponent(id)}/ep${i + 1}.mp4`
        : line === 2
          ? `http://demo.local/media/${encodeURIComponent(id)}/ep${i + 1}.m3u8`
          : `http://demo.local/parse/${encodeURIComponent(id)}?ep=${i + 1}`,
  }))
}

/** 演示样片：真实可播的公开测试流，桌面端 mock 下独立播放器窗口能真实起播 */
const DEMO_SAMPLE_MP4 = 'https://media.w3.org/2010/05/sintel/trailer.mp4'
const DEMO_SAMPLE_HLS = 'https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8'

export function mockResolve(url: string): PlayResult {
  if (!url) throw new Error('解析失败：空地址')
  if (/\.mp4(\?|$)/i.test(url)) {
    return { url: DEMO_SAMPLE_MP4, headers: {}, subtitles: [] }
  }
  if (/\.m3u8(\?|$)/i.test(url) || url.includes('/parse/')) {
    const parsed = url.includes('/parse/')
    return {
      url: DEMO_SAMPLE_HLS,
      headers: {},
      subtitles: [],
      parseName: parsed ? '演示解析' : null,
      trace: ['演示数据：直链判定', parsed ? '演示数据：解析线路直接返回样片' : ''],
    }
  }
  throw new Error('演示数据：无法识别的播放地址')
}
