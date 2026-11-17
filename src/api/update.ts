// 自动更新（Tauri updater 插件）
//
// - 只在桌面端可用：浏览器预览模式下调用会抛出明确错误；
// - 检查地址与公钥在 src-tauri/tauri.conf.json 的 plugins.updater 配置里；
// - 更新包由 CI 用 TAURI_SIGNING_PRIVATE_KEY 签名，客户端用内置公钥校验，
//   签名不匹配的包不会安装。
// - 下载完成后自动重启应用（@tauri-apps/plugin-process 的 relaunch）。

/** 桌面端（Tauri）环境检测：浏览器预览没有 IPC。 */
export const updatesSupported = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/** 可更新的版本信息 */
export interface UpdateInfo {
  version: string
  notes: string
  date: string
}

/** 下载进度（total 未知时为 null） */
export interface UpdateProgress {
  downloaded: number
  total: number | null
}

function requireTauri() {
  if (!updatesSupported) {
    throw new Error('浏览器预览模式不支持自动更新（需桌面端）')
  }
}

/** 当前应用版本：桌面端读 Tauri（与 tauri.conf.json / package.json 同步），预览模式用构建期常量。 */
export async function appVersion(): Promise<string> {
  if (!updatesSupported) return __APP_VERSION__
  const { getVersion } = await import('@tauri-apps/api/app')
  return getVersion()
}

/** 检查更新：有新版本返回 UpdateInfo，已是最新返回 null，网络/配置错误抛异常。 */
export async function checkUpdate(): Promise<UpdateInfo | null> {
  requireTauri()
  const { check } = await import('@tauri-apps/plugin-updater')
  const update = await check()
  if (!update) return null
  return {
    version: update.version,
    notes: update.body ?? '',
    date: update.date ?? '',
  }
}

/**
 * 下载并安装更新，完成后自动重启应用。
 * `onProgress` 会在开始/每次收到分片/完成时回调；安装阶段会关闭当前窗口。
 */
export async function installUpdate(
  onProgress?: (progress: UpdateProgress) => void,
): Promise<void> {
  requireTauri()
  const { check } = await import('@tauri-apps/plugin-updater')
  const update = await check()
  if (!update) throw new Error('当前已是最新版本')
  let downloaded = 0
  let total: number | null = null
  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case 'Started':
        total = event.data.contentLength ?? null
        onProgress?.({ downloaded, total })
        break
      case 'Progress':
        downloaded += event.data.chunkLength
        onProgress?.({ downloaded, total })
        break
      case 'Finished':
        onProgress?.({ downloaded, total: total ?? downloaded })
        break
    }
  })
  const { relaunch } = await import('@tauri-apps/plugin-process')
  await relaunch()
}
