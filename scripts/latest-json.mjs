// 生成 Tauri 自动更新清单 latest.json（在 Release 发布前运行）
//
// 用法：node scripts/latest-json.mjs --dist dist --out dist/latest.json
// 环境变量：
//   GITHUB_REPOSITORY  owner/repo（Actions 自带）
//   TAG                Release tag，例如 v0.1.2（必填）
//   UPDATE_NOTES       更新说明（可选，缺省为空）
//
// 说明：
// - Windows 更新包 = NSIS 安装包本体（`.exe`，Tauri v2），macOS 更新包 = `.app.tar.gz`；
//   两者旁边各有 `.sig`；
// - 任何一个平台缺少产物时跳过该平台；都缺失则不写文件（退出码 0），
//   保证还没配置签名密钥的仓库也能正常跑 CI；
// - 下载 URL 指向 GitHub Release 资产（public repo 可直接下载）。
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const argv = process.argv.slice(2)
let distDir = 'dist'
let outPath = null
for (let i = 0; i < argv.length; i += 1) {
  if (argv[i] === '--dist') distDir = argv[++i]
  else if (argv[i] === '--out') outPath = argv[++i]
}

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const conf = JSON.parse(readFileSync(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'))
const version = conf.version
const repo = process.env.GITHUB_REPOSITORY
const tag = process.env.TAG
if (!repo || !tag) {
  console.error('[latest-json] 缺少 GITHUB_REPOSITORY 或 TAG，跳过')
  process.exit(0)
}

const out = outPath || path.join(distDir, 'latest.json')
const files = existsSync(distDir) ? readdirSync(distDir) : []

/** 读取 `<文件>.sig` 内容（Tauri 生成的签名文件） */
function readSig(file) {
  const sigPath = path.join(distDir, `${file}.sig`)
  return existsSync(sigPath) ? readFileSync(sigPath, 'utf8').trim() : null
}

// Tauri v2 的 Windows 更新包就是 NSIS 安装包本体：url 指向 .exe，signature 取 .exe.sig。
// （v1Compatible 的 .nsis.zip 仍识别，兼容旧产物。）
const PLATFORMS = [
  { key: 'windows-x86_64', suffix: '_x64-setup.exe' },
  { key: 'darwin-aarch64', suffix: '.app.tar.gz' },
]

const platforms = {}
for (const { key, suffix } of PLATFORMS) {
  const file =
    files.find((name) => name.endsWith(suffix)) ??
    (key === 'windows-x86_64' ? files.find((name) => name.endsWith('.nsis.zip')) : undefined)
  if (!file) {
    console.warn(`[latest-json] 跳过 ${key}：dist 里没有 *${suffix}`)
    continue
  }
  const signature = readSig(file)
  if (!signature) {
    console.warn(`[latest-json] 跳过 ${key}：缺少 ${file}.sig`)
    continue
  }
  platforms[key] = {
    signature,
    url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`,
  }
}

if (!Object.keys(platforms).length) {
  console.warn('[latest-json] 没有任何可发布的更新包（未配置签名密钥？），不生成 latest.json')
  process.exit(0)
}

const payload = {
  version,
  notes: process.env.UPDATE_NOTES || '',
  pub_date: new Date().toISOString(),
  platforms,
}
writeFileSync(out, `${JSON.stringify(payload, null, 2)}\n`, 'utf8')
console.log(`[latest-json] 已生成 ${out}（version=${version}，平台：${Object.keys(platforms).join(', ')}）`)
