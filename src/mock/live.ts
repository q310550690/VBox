// 直播能力演示数据（mock）：分组 / 频道 / 节目单
// 直播解析（live.rs）与 EPG 尚未实现，页面必须显示「演示数据 / 待接入」标识。
import type { EpgProgram, LiveChannel, LiveGroup } from '@/types'

export const DEMO_LIVE_GROUPS: LiveGroup[] = [
  { key: 'cctv', name: '央视' },
  { key: 'satellite', name: '卫视' },
  { key: 'local', name: '地方台' },
  { key: 'hkmotw', name: '港澳台' },
  { key: 'movie', name: '影视轮播' },
  { key: 'sports', name: '体育赛事' },
]

const CHANNELS: Record<string, string[]> = {
  cctv: ['CCTV-1 综合', 'CCTV-2 财经', 'CCTV-5 体育', 'CCTV-6 电影', 'CCTV-8 电视剧', 'CCTV-13 新闻'],
  satellite: ['湖南卫视', '浙江卫视', '江苏卫视', '东方卫视', '北京卫视', '广东卫视'],
  local: ['本地新闻综合', '本地生活', '本地影视', '本地少儿', '本地公共', '本地交通'],
  hkmotw: ['翡翠台', '明珠台', 'TVB 新闻', '中天新闻', '东森电影', '凤凰中文'],
  movie: ['经典电影', '动作影院', '科幻影院', '爱情影院', '喜剧影院', '深夜影院'],
  sports: ['体育 1', '体育 2', '足球频道', '篮球频道', '赛车频道', '网球频道'],
}

export function mockLiveChannels(groupKey: string): LiveChannel[] {
  const names = CHANNELS[groupKey] ?? []
  return names.map((name, i) => ({
    id: `${groupKey}-${i + 1}`,
    name,
    groupKey,
    logo: null,
    nowPlaying: '演示节目 · 节目单待接入',
    nextPlaying: null,
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
