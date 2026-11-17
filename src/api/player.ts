// 播放控制封装（对应 Rust 命令 play / player_control）
import { invoke } from '@tauri-apps/api/core'

// 开始播放某个直链
export function play(url: string, config = '') {
  return invoke('play', { url, config })
}

// 播放控制动作类型（与 Rust 端 player_control 对应）
export type PlayerAction = 'play' | 'pause' | 'stop' | 'seek' | 'volume'

export function playerControl(action: PlayerAction | string) {
  return invoke('player_control', { action })
}
