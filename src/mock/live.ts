// 直播能力演示数据（mock）：分组 / 频道 / 节目单（2026-11-25 扩充）
// 直播解析（live.rs）与 EPG 尚未实现，页面必须显示「演示数据 / 待接入」标识。
import type { EpgProgram, LiveChannel, LiveGroup } from '@/types'

export const DEMO_LIVE_GROUPS: LiveGroup[] = [
  { key: 'cctv', name: '央视' },
  { key: 'satellite', name: '卫视' },
  { key: 'local', name: '地方台' },
  { key: 'hkmotw', name: '港澳台' },
  { key: 'movie', name: '影视轮播' },
  { key: 'sports', name: '体育赛事' },
  { key: 'kids', name: '少儿动画' },
  { key: 'music', name: '音乐现场' },
]

const CHANNELS: Record<string, string[]> = {
  cctv: ['CCTV-1 综合', 'CCTV-2 财经', 'CCTV-3 综艺', 'CCTV-5 体育', 'CCTV-5+ 体育赛事', 'CCTV-6 电影', 'CCTV-8 电视剧', 'CCTV-9 纪录', 'CCTV-10 科教', 'CCTV-13 新闻'],
  satellite: ['湖南卫视', '浙江卫视', '江苏卫视', '东方卫视', '北京卫视', '广东卫视', '深圳卫视', '安徽卫视', '山东卫视', '天津卫视'],
  local: ['本地新闻综合', '本地生活', '本地影视', '本地少儿', '本地公共', '本地交通', '本地体育', '本地戏曲'],
  hkmotw: ['翡翠台', '明珠台', 'TVB 新闻', 'TVB 财经', '中天新闻', '东森电影', '东森新闻', '凤凰中文', '凤凰资讯', '澳视澳门'],
  movie: ['经典电影', '动作影院', '科幻影院', '爱情影院', '喜剧影院', '深夜影院', '港片经典', '环球影院'],
  sports: ['体育 1', '体育 2', '足球频道', '篮球频道', '赛车频道', '网球频道', '羽毛球频道', '电竞频道'],
  kids: ['少儿动画', '卡酷少儿', '金鹰卡通', '哈哈少儿', '优漫卡通', '嘉佳卡通'],
  music: ['音乐现场', '华语金曲', '欧美热榜', '日韩音乐', '怀旧金曲', '演唱会直播'],
}

const NOW_PLAYING = [
  '演示节目 · 新闻直播间',
  '演示节目 · 热播剧场',
  '演示节目 · 晚间综艺',
  '演示节目 · 动画片场',
  '演示节目 · 赛事直播',
  '演示节目 · 电影放送',
  '演示节目 · 音乐现场',
  '演示节目 · 纪录片',
]

const NEXT_PLAYING = [
  '预告 · 下一档节目',
  '预告 · 黄金档剧场',
  '预告 · 深夜影院',
  null,
]

export function mockLiveChannels(groupKey: string): LiveChannel[] {
  const names = CHANNELS[groupKey] ?? []
  return names.map((name, i) => ({
    id: `${groupKey}-${i + 1}`,
    name,
    groupKey,
    logo: null,
    nowPlaying: NOW_PLAYING[i % NOW_PLAYING.length],
    nextPlaying: NEXT_PLAYING[i % NEXT_PLAYING.length],
  }))
}

export function mockLiveGroups(): LiveGroup[] {
  return DEMO_LIVE_GROUPS
}

/** EPG 未实现：返回空数组，页面显示「节目单待接入」占位文案（不编造时间） */
export function mockEpg(_channelId: string): EpgProgram[] {
  void _channelId
  return []
}
