// 打包前确保选中的原生依赖已经「随包就位」。
//
// 由 tauri.conf.json 的 beforeBuildCommand 调用（`npm run build && npm run ensure:tools`）。
// 选择哪些工具内置由 `VBOX_BUNDLE` 决定：
//   - 不设置 / "all"        → ffmpeg,python,node（默认全内置）
//   - "none" / ""           → 都不内置（极简安装包；相关功能回退系统/外部工具）
//   - "ffmpeg,node"          → 只内置这两项
//
// 缺失项交给对应 scripts/fetch-*.ps1 下载（固定版本 + SHA-256）；失败就让构建失败，
// 避免打出「看似内置、实际没有」的包。
import { existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
export const TOOLS = {
  ffmpeg: {
    label: 'FFmpeg',
    marker: 'src-tauri/resources/ffmpeg/ffmpeg.exe',
    script: 'scripts/fetch-ffmpeg.ps1',
  },
  python: {
    label: 'Python',
    // 用完成标记而不是 python.exe：避免把「只解压、依赖没装完」的半成品打进包里
    marker: 'src-tauri/resources/python/.vbox-ready',
    script: 'scripts/fetch-python.ps1',
  },
  jvm: {
    label: 'JRE 17 (jar spiders)',
    // jar 蛛蛛要真 JVM：先用 build-jvm.mjs 产出兼容层 jar，再下 JRE 17
    marker: 'src-tauri/resources/jvm/marker.json',
    build: 'scripts/build-jvm.mjs',
    script: 'scripts/fetch-jre.ps1',
  },
  node: {
    label: 'Node.js',
    marker: 'src-tauri/resources/node/.vbox-ready',
    script: 'scripts/fetch-node.ps1',
  },
}

/** 解析 VBOX_BUNDLE：未设置/all=全部，none 或空串=不内置，逗号分隔=指定项。 */
export function parseBundle(raw) {
  if (raw === undefined || raw === null) return Object.keys(TOOLS)
  const value = raw.trim().toLowerCase()
  if (value === 'all') return Object.keys(TOOLS)
  if (value === '' || value === 'none') return []
  const names = value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
  const unknown = names.filter((name) => !TOOLS[name])
  if (unknown.length) {
    throw new Error(`未知的工具：${unknown.join(', ')}（可选：${Object.keys(TOOLS).join(', ')}）`)
  }
  return [...new Set(names)]
}

let shellCommand
/** 优先 pwsh（GitHub Actions Windows runner 一定有），本机没有时回退 powershell。 */
function powershellCommand() {
  if (shellCommand) return shellCommand
  for (const candidate of ['pwsh', 'powershell']) {
    const probe = spawnSync(candidate, ['-NoProfile', '-Command', 'exit 0'], { stdio: 'ignore' })
    if (!probe.error && probe.status === 0) {
      shellCommand = candidate
      return candidate
    }
  }
  shellCommand = 'powershell'
  return shellCommand
}

function run(script, args) {
  return spawnSync(
    powershellCommand(),
    ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(root, script), ...args],
    { cwd: root, stdio: 'inherit' },
  )
}

/** 确保选中工具已就位；返回成功与否。 */
export function ensureTools(selection) {
  if (process.platform !== 'win32') {
    console.warn('[ensure-tools] 非 Windows 平台跳过随包运行时；用户可用系统安装的版本')
    return true
  }
  for (const name of selection) {
    const tool = TOOLS[name]
    const marker = path.join(root, tool.marker)
    if (existsSync(marker)) {
      console.log(`[ensure-tools] ${tool.label} 已就位（随安装包分发）`)
      continue
    }
    if (tool.build) {
      console.log(`[ensure-tools] ${tool.label} 缺兼容层 jar，先构建…`)
      const built = spawnSync(process.execPath, [path.join(root, tool.build)], {
        cwd: root,
        stdio: 'inherit',
      })
      if (built.error || built.status !== 0) {
        console.error(`[ensure-tools] ${tool.build} 构建失败`)
        return false
      }
    }
    console.log(`[ensure-tools] ${tool.label}的运行时缺失，开始下载…`)
    const result = run(tool.script, [])
    if (result.error) {
      console.error(`[ensure-tools] 启动 ${tool.script} 失败：${result.error.message}`)
      return false
    }
    if (result.status !== 0 || !existsSync(marker)) {
      console.error(`[ensure-tools] ${tool.label} 下载失败；可手动运行脚本后重试`)
      return false
    }
  }
  return true
}

// 直接被 `node scripts/ensure-tools.mjs` / `npm run ensure:tools` 调用时执行
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let selection
  try {
    selection = parseBundle(process.env.VBOX_BUNDLE)
  } catch (err) {
    console.error(`[ensure-tools] ${err.message}`)
    process.exit(1)
  }
  console.log(
    selection.length
      ? `[ensure-tools] 内置：${selection.map((name) => TOOLS[name].label).join(' + ')}`
      : '[ensure-tools] 本次不内置任何外部工具（极简包）',
  )
  process.exit(ensureTools(selection) ? 0 : 1)
}
