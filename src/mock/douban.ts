// 豆瓣首页演示数据（仅浏览器预览 / 后端不可用时使用）
// 约定：mock 数据只在 src/api 门面里使用；页面会展示「演示数据」标识。
// 真实桌面端由后端 douban_home 命令抓取豆瓣：热门电影 / 热门剧集 / 热门综艺 / 热门动漫 /
// TOP250 / 影院热映 / 即将上映 / 纪录片，并为本机图片代理注入 imageProxy。
import type {
  DoubanCollection,
  DoubanHome,
  DoubanItem,
  DoubanPerson,
  DoubanSubjectDetail,
  DoubanSubjectMeta,
} from '@/types'

interface CollectionSeed {
  key: string
  name: string
  titles: string[]
  /** 地区 / 类型，用于拼 info */
  region: string
  genres: string
  /** 豆瓣条目类型 */
  subtype: string
}

const MOVIE_TITLES = [
  '长夜将明', '山海之间', '雾城疑云', '流浪星球', '盛夏回声', '无声告白',
  '风起长安', '深海纪元', '人生海海', '星轨之外', '雪落长街', '幻境棋局',
  '末日余晖', '春日列车', '归途有风', '千面之城', '月下孤影', '时间的形状',
  '荒野来信', '第七个黎明', '南方以南', '燃烧的平原', '蓝调时刻', '静止的河流',
]

const TV_TITLES = [
  '小城良方', '长街烟火', '山河故人', '暗潮之下', '南风知我意', '温柔的代价',
  '长安十二时', '沉默的真相', '看不见的客人', '向风而行', '烟火人间', '逆流而上',
  '旧巷子里的人', '星光不问赶路人', '深海之光', '与你同频', '迷雾追踪', '未完成的夏天',
  '远方来信', '岁月无声', '海边的曼彻斯特', '第七封信', '风平浪静', '白日梦我',
]

const SHOW_TITLES = [
  '后会友期', '快乐再出发', '我们的歌', '声生不息', '一年一度喜剧大赛', '奔跑吧',
  '王牌对王牌', '极限挑战', '花儿与少年', '向往的生活', '脱口秀大会', '中国好声音',
  '天赐的声音', '乘风破浪', '披荆斩棘', '歌手', '新生日记', '无限超越班',
  '舞台新声', '三餐四季', '旅行的意义', '一起露营吧', '戏剧新生活', '沸腾吧火锅',
]

const ANIME_TITLES = [
  '时光代理人', '雾山五行', '灵笼', '凡人修仙传', '斗罗大陆', '吞噬星空',
  '沧元图', '完美世界', '遮天', '一念永恒', '星辰变', '武动乾坤',
  '枕刀歌', '罗小黑战记', '刺客伍六七', '万国志', '少年歌行', '眷思量',
  '大鱼海棠', '深海', '白蛇：浮生', '茶啊二中', '雄狮少年', '肆式青春',
]

const TOP250_TITLES = [
  '肖申克的救赎', '霸王别姬', '阿甘正传', '泰坦尼克号', '千与千寻', '这个杀手不太冷',
  '美丽人生', '星际穿越', '盗梦空间', '楚门的世界', '辛德勒的名单', '忠犬八公的故事',
  '放牛班的春天', '海上钢琴师', '三傻大闹宝莱坞', '机器人总动员', '无间道', '疯狂动物城',
  '大话西游', '龙猫', '教父', '当幸福来敲门', '怦然心动', '触不可及',
]

const SHOWING_TITLES = [
  '黑球', '逃出布迪秀', '野马九号', '风起大漠', '深海救援', '最后一公里',
  '雪国列车', '逆光飞翔', '无界之地', '城市之光', '末日方舟', '非常行动',
  '绝地反击', '山海奇缘', '少年的海', '长安夜行', '星河入梦', '烈日灼心',
  '无声之战', '破晓时分', '荒野猎人', '空中之城', '静默的河', '风中有朵雨做的云',
]

const SOON_TITLES = [
  '时间之子', '云边有个小卖部', '看不见的朋友', '孤注一掷', '三大队', '拯救嫌疑人',
  '河边的错误', '刀尖', '热搜', '拯救计划', '熊出没·逆转时空', '红毯先生',
  '草木人间', '被我弄丢的你', '黄雀在后', '白蛇：浮生', '负负得正', '假如爱情欺骗了你',
  '野孩子', '出入平安', '只此青绿', '749局', '危机航线', '志愿军',
]

const DOC_TITLES = [
  '人生七年10', '我在故宫修文物', '蓝色星球', '地球脉动', '河西走廊', '舌尖上的中国',
  '大国重器', '航拍中国', '宇宙时空之旅', '人类星球', '冰冻星球', '美丽中国',
  '了不起的村落', '风味人间', '世界遗产', '自然的力量', '极地', '秘境之眼',
  '城市24小时', '中国医生', '守护解放西', '但是还有书籍', '生命之源', '大地之吻',
]

const COLLECTION_SEEDS: CollectionSeed[] = [
  { key: 'movie_hot_gaia', name: '热门电影', titles: MOVIE_TITLES, region: '中国大陆', genres: '剧情 喜剧', subtype: 'movie' },
  { key: 'tv_hot', name: '热门剧集', titles: TV_TITLES, region: '中国大陆', genres: '剧情', subtype: 'tv' },
  { key: 'show_hot', name: '热门综艺', titles: SHOW_TITLES, region: '中国大陆', genres: '真人秀 综艺', subtype: 'show' },
  { key: 'tv_animation', name: '热门动漫', titles: ANIME_TITLES, region: '日本 中国大陆', genres: '动画 奇幻', subtype: 'tv' },
  { key: 'movie_top250', name: 'TOP250', titles: TOP250_TITLES, region: '美国', genres: '剧情', subtype: 'movie' },
  { key: 'movie_showing', name: '影院热映', titles: SHOWING_TITLES, region: '中国大陆', genres: '动作 悬疑', subtype: 'movie' },
  { key: 'movie_soon', name: '即将上映', titles: SOON_TITLES, region: '中国大陆', genres: '剧情 爱情', subtype: 'movie' },
  { key: 'tv_documentary', name: '纪录片', titles: DOC_TITLES, region: '中国大陆', genres: '纪录片', subtype: 'tv' },
]

const DIRECTORS = ['张晓波', '陈凯歌', '娄烨', '贾樟柯', '刁亦男', '文牧野', '饶晓志', '曹保平']
const ACTORS = ['肖战 倪大红', '张译 秦海璐', '周迅 黄渤', '易烊千玺 刘浩存', '雷佳音 杨幂', '朱一龙 倪妮']

/** 固定种子评分：稳定、可复现（7.2 - 9.3） */
function ratingOf(index: number): number {
  return Math.round((7.2 + ((index * 37) % 22) / 10) * 10) / 10
}

function makeItem(seed: CollectionSeed, index: number): DoubanItem {
  const title = seed.titles[index % seed.titles.length]
  const director = DIRECTORS[index % DIRECTORS.length]
  const actors = ACTORS[index % ACTORS.length]
  const year = String(2020 + ((index * 3) % 7))
  return {
    id: `demo-${seed.key}-${index}`,
    title,
    cover: `https://picsum.photos/seed/vbox-douban-${seed.key}-${index}/400/560`,
    rating: ratingOf(index),
    year,
    info: `${seed.region} / ${seed.genres} / ${director} / ${actors}`,
    // 介绍（剧情简介）：卡片第三行展示，超出省略
    intro: `${INTROS[index % INTROS.length]}`,
    subtype: seed.subtype,
  }
}

const INTROS = [
  '一段关于选择与告别的故事。当命运的岔路突然出现，几个原本毫无交集的人被推到一起，他们必须在有限的时间里做出决定，也重新认识了自己。',
  '影片以细腻的镜头语言描摹普通人的日常，在琐碎的生活缝隙里，藏着最动人的情感与最坚韧的力量。',
  '当真相被层层迷雾包裹，追查者一步步逼近核心，却发现最难以面对的，是自己心底那个不愿承认的答案。',
  '一场跨越十年的约定，把散落各地的人重新聚拢。旧照片、老唱片和那条熟悉的长街，都在提醒他们：有些东西从未改变。',
  '在城市的边缘，一群普通人用自己的方式守护着彼此。他们的故事没有惊天动地，却足以照亮漫长黑夜。',
]

/** 轮播：热门电影前 5 + 热门剧集前 5，补剧情简介 */
function makeCarousel(collections: DoubanCollection[]): DoubanItem[] {
  const movies = collections.find((c) => c.key === 'movie_hot_gaia')?.items.slice(0, 6) ?? []
  const tvs = collections.find((c) => c.key === 'tv_hot')?.items.slice(0, 6) ?? []
  return [...movies, ...tvs].map((item, index) => ({
    ...item,
    intro: INTROS[index % INTROS.length],
  }))
}

/** 浏览器预览用：生成一份稳定的豆瓣首页演示数据 */
export function mockDoubanHome(_force = false): DoubanHome {
  const collections = COLLECTION_SEEDS.map((seed): DoubanCollection => ({
    key: seed.key,
    name: seed.name,
    items: Array.from({ length: 30 }, (_, index) => makeItem(seed, index)),
  }))
  return {
    carousel: makeCarousel(collections),
    collections,
    fetchedAt: Date.now(),
    cacheHit: false,
    imageProxy: null,
  }
}

/* ---------------- 豆瓣主创（演员照片）演示数据 ---------------- */
// 桌面端由 Rust `douban_people` 实时抓取豆瓣；浏览器预览用此处演示数据展示演员照片行。
// 头像使用公开人像占位服务（pravatar），断网时组件会退回首字圆形占位。

const PEOPLE: Array<{ name: string; role: string }> = [
  { name: '张译', role: '演员' },
  { name: '周迅', role: '演员' },
  { name: '黄渤', role: '演员' },
  { name: '秦海璐', role: '演员' },
  { name: '雷佳音', role: '演员' },
  { name: '倪妮', role: '演员' },
  { name: '易烊千玺', role: '演员' },
  { name: '张子枫', role: '演员' },
  { name: '王传君', role: '演员' },
  { name: '章宇', role: '演员' },
  { name: '沈腾', role: '演员' },
  { name: '马丽', role: '演员' },
  { name: '吴京', role: '演员' },
  { name: '段奕宏', role: '演员' },
  { name: '廖凡', role: '演员' },
  { name: '桂纶镁', role: '演员' },
  { name: '胡歌', role: '演员' },
  { name: '刘亦菲', role: '演员' },
  { name: '陈凯歌', role: '导演' },
  { name: '贾樟柯', role: '导演' },
  { name: '郭帆', role: '导演' },
  { name: '宁浩', role: '导演' },
  { name: '文牧野', role: '导演' },
  { name: '娄烨', role: '导演' },
]

function hashText(text: string): number {
  let h = 0
  for (let i = 0; i < text.length; i += 1) h = (h * 31 + text.charCodeAt(i)) % 100000
  return h
}

/** 浏览器预览：按片名稳定生成 12 位主创（2 位导演 + 10 位演员），带真人头像地址 */
export function mockDoubanPeople(title: string, year?: string | null): DoubanPerson[] {
  const base = hashText(`${title}|${year ?? ''}`)
  const list: DoubanPerson[] = []
  const used = new Set<number>()
  let cursor = base % PEOPLE.length
  while (list.length < 12) {
    if (!used.has(cursor)) {
      used.add(cursor)
      const person = PEOPLE[cursor]
      list.push({
        name: person.name,
        role: person.role,
        avatar: `https://i.pravatar.cc/160?img=${(base + list.length * 17) % 70 + 1}`,
      })
    }
    cursor = (cursor + 1) % PEOPLE.length
  }
  return list
}

/** 浏览器预览：按片名稳定生成剧照（picsum 16:9，最多 4 张） */
export function mockDoubanStills(title: string, year?: string | null, count = 4): string[] {
  const base = hashText(`${title}|${year ?? ''}`)
  return Array.from(
    { length: count },
    (_, i) => `https://picsum.photos/seed/vbox-still-${base}-${i}/640/360`,
  )
}

const MOCK_GENRES = ['剧情', '动画', '奇幻', '动作', '喜剧', '悬疑']
const MOCK_REGIONS = ['中国大陆', '中国香港', '日本', '美国', '韩国']

/** 浏览器预览：按片名稳定生成影视信息（评分 / 类型 / 上映 / 片长 / 地区 / 语言） */
export function mockDoubanMeta(title: string, year?: string | null): DoubanSubjectMeta {
  const base = hashText(`${title}|${year ?? ''}`)
  const y = Number(year ?? '') || 2015 + (base % 12)
  return {
    rating: (6.8 + (base % 30) / 10).toFixed(1),
    ratingCount: 12_000 + (base % 98_000),
    genres: [MOCK_GENRES[base % MOCK_GENRES.length], MOCK_GENRES[(base + 2) % MOCK_GENRES.length]],
    pubdate: `${y}-${String((base % 12) + 1).padStart(2, '0')}-${String((base % 28) + 1).padStart(2, '0')}`,
    durations: [`${100 + (base % 40)}分钟`],
    countries: [MOCK_REGIONS[base % MOCK_REGIONS.length]],
    languages: ['汉语普通话'],
  }
}

/** 浏览器预览：详情页豆瓣扩展数据（主创 + 剧照 + 影视信息） */
export function mockDoubanDetail(title: string, year?: string | null): DoubanSubjectDetail {
  return {
    people: mockDoubanPeople(title, year),
    stills: mockDoubanStills(title, year),
    meta: mockDoubanMeta(title, year),
  }
}
