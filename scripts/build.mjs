// `npm run tauri:build` 的入口：打包时可选择内置哪些原生依赖。
//
// 用法（参数会透传给 `tauri build`，除了 --bundle）：
//   npm run tauri:build                          # 默认全内置（ffmpeg+python+node）
//   npm run tauri:build -- --bundle none         # 极简包，不内置任何外部工具
//   npm run tauri:build -- --bundle ffmpeg,node  # 只内置 FFmpeg 和 Node
//   npm run tauri:build -- --bundle python --debug
//
// 实现：把当前 tauri.conf.json 复制成 src-tauri/tauri.build.conf.json，
// 只保留选中工具的 resources，再让 Tauri CLI 用这份配置构建；
// 这样「不内置」既不影响已下载的文件，也不需要每次重新下载。
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { TOOLS, parseBundle } from './ensure-tools.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

// GitHub Actions 会把未配置的 secret 注入成空字符串。Tauri 只要看到
// APPLE_CERTIFICATE 存在（哪怕是空串）就会尝试导入证书，macOS 打包会在
// codesign 阶段报 “Unable to decode the provided data”。这里清掉空值，
// 让未配置证书时正常走 ad-hoc 签名。
for (const key of [
  'APPLE_CERTIFICATE',
  'APPLE_CERTIFICATE_PASSWORD',
  'APPLE_SIGNING_IDENTITY',
  'APPLE_ID',
  'APPLE_PASSWORD',
  'APPLE_TEAM_ID',
]) {
  if (process.env[key] === '') delete process.env[key]
}

const argv = process.argv.slice(2)
let bundleRaw = process.env.VBOX_BUNDLE
let dryRun = false
const passthrough = []
for (let i = 0; i < argv.length; i += 1) {
  const arg = argv[i]
  if (arg === '--dry-run') {
    dryRun = true
  } else if (arg === '--bundle' || arg === '-b') {
    bundleRaw = argv[i + 1]
    i += 1
    if (bundleRaw === undefined) {
      console.error('用法：npm run tauri:build -- --bundle <all|none|ffmpeg,python,node>')
      process.exit(1)
    }
  } else if (arg.startsWith('--bundle=')) {
    bundleRaw = arg.slice('--bundle='.length)
  } else {
    passthrough.push(arg)
  }
}

let selection
try {
  selection = parseBundle(bundleRaw)
} catch (err) {
  console.error(`[build] ${err.message}`)
  process.exit(1)
}

// 内置的 ffmpeg/python/node 目前都是 Windows 产物；macOS/Linux 先走系统安装。
if (process.platform !== 'win32' && selection.length) {
  console.warn(
    `[build] 当前平台（${process.platform}）暂不支持内置 FFmpeg/Python/Node（产物只有 Windows 版），已忽略选择`,
  )
  selection = []
}

const confPath = path.join(root, 'src-tauri', 'tauri.conf.json')
const conf = JSON.parse(readFileSync(confPath, 'utf8'))
// Tauri 用 RFC 7386（json_patch::merge）把 --config 合并到 tauri.conf.json：
// 对象递归合并、**数组整体替换**、null 删除键。所以这里用数组写「选中项」，
// 生成配置里的数组会直接替换 base 的三项，实现「不内置」而无需删键。
// macOS 侧用 map（tauri.macos.conf.json 也是 map），避免数组/对象类型冲突。
const resources =
  process.platform === 'win32'
    ? ['resources/libmpv-2.dll']
    : process.platform === 'darwin'
      ? { 'resources/libmpv.dylib': 'resources/libmpv.dylib' }
      : []
for (const name of selection) {
  if (Array.isArray(resources)) resources.push(`resources/${name}/`)
}
conf.bundle = { ...conf.bundle, resources }

// 自动更新：CI 注入签名私钥后才生成 updater 产物（Windows `.nsis.zip` /
// macOS `.app.tar.gz` + `.sig`）。本地普通打包不设该变量，免得每次都要求密钥。
if (process.env.TAURI_SIGNING_PRIVATE_KEY || process.env.TAURI_SIGNING_PRIVATE_KEY_PATH) {
  conf.bundle.createUpdaterArtifacts = true
}

// Windows Authenticode 代码签名：CI 把 .pfx 导入证书库后导出指纹，
// 这里写入构建配置（本地未配置证书时保持不签名）。
if (process.env.WINDOWS_CERT_THUMBPRINT) {
  conf.bundle.windows = {
    ...(conf.bundle.windows || {}),
    certificateThumbprint: process.env.WINDOWS_CERT_THUMBPRINT,
    digestAlgorithm: 'sha256',
    timestampUrl: process.env.WINDOWS_TIMESTAMP_URL || 'http://timestamp.digicert.com',
  }
}

const outPath = path.join(root, 'src-tauri', 'tauri.build.conf.json')
writeFileSync(outPath, `${JSON.stringify(conf, null, 2)}\n`, 'utf8')

console.log(
  selection.length
    ? `[build] 内置：${selection.map((name) => TOOLS[name].label).join(' + ')}`
    : '[build] 不内置任何外部工具（极简包）',
)
console.log(`[build] 生成配置：${path.relative(root, outPath)}`)

if (dryRun) {
  console.log('[build] --dry-run：只生成配置，不调用 Tauri CLI')
  process.exit(0)
}

const cli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
if (!existsSync(cli)) {
  console.error(`[build] 找不到 Tauri CLI：${cli}（先 npm install）`)
  process.exit(1)
}

const env = {
  ...process.env,
  // 不能用空串表示「不内置」：ensure-tools 里空串按「没有设置」会回退成全内置。
  VBOX_BUNDLE: selection.length ? selection.join(',') : 'none',
}
const result = spawnSync(
  process.execPath,
  [cli, 'build', '--config', path.relative(root, outPath), ...passthrough],
  { cwd: root, stdio: 'inherit', env },
)
if (result.error) {
  console.error(`[build] 启动 Tauri CLI 失败：${result.error.message}`)
  process.exit(1)
}
process.exit(result.status ?? 1)
