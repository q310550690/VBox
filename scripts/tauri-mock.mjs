#!/usr/bin/env node
/**
 * 以 mock 数据启动 Tauri 桌面端（前端 demoMode）。
 *
 * 用法：pnpm tauri:mock
 * 原理：注入 VITE_DEMO=1 后启动 `tauri dev`；Vite 读到后把 demoMode 置为 true，
 *       订阅/点播/搜索/豆瓣/直播/历史/收藏/下载全部走 src/mock 演示数据，
 *       独立播放器窗口仍走真实 mpv + Rust 命令（播放公开测试样片）。
 */
import { spawn } from 'node:child_process'

const win = process.platform === 'win32'
const env = { ...process.env, VITE_DEMO: '1' }
const child = spawn(win ? 'pnpm.cmd' : 'pnpm', ['tauri', 'dev'], {
  stdio: 'inherit',
  env,
  shell: win,
})

child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exit(code ?? 0)
})
