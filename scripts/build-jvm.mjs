// 构建 JVM 车道运行时：编译 stubs.jar / spider-host.jar，并把 Maven 依赖整理进 resources/jvm。
//
// 用法：
//   node scripts/build-jvm.mjs            全量构建
//   node scripts/build-jvm.mjs --check    只编译（不打包、不落 resources，CI 快速校验）
//   node scripts/build-jvm.mjs --clean    先清空中间目录
//
// 前置：
//   - JDK 17+（javac / jar）。用 VBOX_JDK_HOME 或 JAVA_HOME 指定，否则从 PATH 与常见目录探测。
//   - jvm/vendor/*.jar（缺失时本脚本自动下载，固定 sha256）
//
// 产物（src-tauri/resources/jvm）：
//   stubs.jar / spider-host.jar / libs/*.jar / manifest.json
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, copyFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import crypto from 'node:crypto'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const UP = path.join(root, 'jvm', 'upstream', 'winbox')
const HOST_SRC = path.join(root, 'jvm', 'host', 'src')
const COMPAT_SRC = path.join(root, 'jvm', 'compat', 'src')
const VENDOR = path.join(root, 'jvm', 'vendor')
const BUILD = path.join(root, 'jvm', 'build')
const OUT = path.join(root, 'src-tauri', 'resources', 'jvm')
const RELEASE = '17'

const argv = process.argv.slice(2)
const checkOnly = argv.includes('--check')
const clean = argv.includes('--clean')

function die(msg) {
  console.error(`[build-jvm] ${msg}`)
  process.exit(1)
}
function log(msg) {
  console.log(`[build-jvm] ${msg}`)
}

function walk(dir, filter) {
  const out = []
  for (const name of readdirSync(dir)) {
    const full = path.join(dir, name)
    const st = statSync(full)
    if (st.isDirectory()) out.push(...walk(full, filter))
    else if (!filter || filter(full)) out.push(full)
  }
  return out
}

function sha256(file) {
  return crypto.createHash('sha256').update(readFileSync(file)).digest('hex')
}

function findJdk() {
  const homeCandidates = [
    process.env.VBOX_JDK_HOME,
    process.env.JAVA_HOME,
    'C:/Program Files/Eclipse Adoptium/jdk-17',
    'C:/Program Files/Java/jdk-17',
    'D:/Software/Development/Java/21',
    '/usr/lib/jvm/default-java',
    '/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home',
  ].filter(Boolean)
  for (const home of homeCandidates) {
    const javac = path.join(home, 'bin', process.platform === 'win32' ? 'javac.exe' : 'javac')
    if (existsSync(javac)) return { javac, jar: path.join(home, 'bin', process.platform === 'win32' ? 'jar.exe' : 'jar') }
  }
  const probe = spawnSync(process.platform === 'win32' ? 'javac.exe' : 'javac', ['-version'], { encoding: 'utf8' })
  if (!probe.error && probe.status === 0) {
    const jarProbe = spawnSync(process.platform === 'win32' ? 'jar.exe' : 'jar', ['--version'], { encoding: 'utf8' })
    if (!jarProbe.error && jarProbe.status === 0) return { javac: 'javac', jar: 'jar' }
  }
  die('未找到 JDK（需要 17+）。请设置 VBOX_JDK_HOME 或 JAVA_HOME，或把 javac 放进 PATH。')
}

function run(cmd, args, label) {
  const res = spawnSync(cmd, args, { stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' })
  if (res.error) die(`${label} 启动失败：${res.error.message}`)
  if (res.status !== 0) {
    const detail = [res.stdout, res.stderr].filter(Boolean).join('\n').trim()
    die(`${label} 失败（exit ${res.status}）\n${detail}`)
  }
  return [res.stdout, res.stderr].filter(Boolean).join('\n').trim()
}

const jdk = findJdk()
const versionText = run(jdk.javac, ['-version'], 'javac -version')
const versionMatch = /javac\s+(\d+)/.exec(versionText)
const major = versionMatch ? Number(versionMatch[1]) : 0
if (major < 17) die(`需要 JDK 17+，当前：${versionText}`)
log(`JDK: ${versionText} (${jdk.javac})`)

// ---- Maven 依赖（固定版本 + SHA-256；缺什么下什么，镜像回退）----
const MAVEN_LIBS = [
  { name: 'bcprov-jdk18on-1.78.1.jar', path: 'org/bouncycastle/bcprov-jdk18on/1.78.1/bcprov-jdk18on-1.78.1.jar', sha256: 'add5915e6acfc6ab5836e1fd8a5e21c6488536a8c1f21f386eeb3bf280b702d7', bytes: 8324412 },
  { name: 'gson-2.10.1.jar', path: 'com/google/code/gson/gson/2.10.1/gson-2.10.1.jar', sha256: '4241c14a7727c34feea6507ec801318a3d4a90f070e4525681079fb94ee4c593', bytes: 283367 },
  { name: 'json-20180813.jar', path: 'org/json/json/20180813/json-20180813.jar', sha256: '518080049ba83181914419d11a25d9bc9833a2d729b6a6e7469fa52851356da8', bytes: 64597 },
  { name: 'jsoup-1.17.2.jar', path: 'org/jsoup/jsoup/1.17.2/jsoup-1.17.2.jar', sha256: 'f60b33b38e9d7ac93eaaa68a6c70f706bb99036494b2e2add2bfee11d09ac6f5', bytes: 445706 },
  { name: 'okhttp-3.14.9.jar', path: 'com/squareup/okhttp3/okhttp/3.14.9/okhttp-3.14.9.jar', sha256: '2570fab55515cbf881d7a4ceef49fc515490bc027057e666776a2832465aeca0', bytes: 430488 },
  { name: 'okio-1.17.5.jar', path: 'com/squareup/okio/okio/1.17.5/okio-1.17.5.jar', sha256: '19a7ff48d86d3cf4497f7f250fbf295f430c13a528dd5b7b203f821802b886ad', bytes: 92326 },
  { name: 'slf4j-api-2.0.16.jar', path: 'org/slf4j/slf4j-api/2.0.16/slf4j-api-2.0.16.jar', sha256: 'a12578dde1ba00bd9b816d388a0b879928d00bab3c83c240f7013bf4196c579a', bytes: 69435 },
  { name: 'slf4j-simple-2.0.16.jar', path: 'org/slf4j/slf4j-simple/2.0.16/slf4j-simple-2.0.16.jar', sha256: 'effc32018658bea09d1e08c7d1060ccad46c086960f583d07dd7ffe9c1172a47', bytes: 15704 },
  { name: 'sqlite-jdbc-3.46.1.0.jar', path: 'org/xerial/sqlite-jdbc/3.46.1.0/sqlite-jdbc-3.46.1.0.jar', sha256: '6dc7464e3803648d3ff18a7359bab6adf079fcd8495b18991f6f5edcb8ac6e3b', bytes: 14123618 },
]


// ---- unidbg（ARM32/AArch64 .so 模拟执行，Win-box 的 native bridge 依赖它）----
// 版本钉 0.9.9：Win-box 的 native-bridge 就是按这个版本的 API 写的。
const UNIDBG_VENDOR = path.join(root, 'jvm', 'vendor', 'unidbg')
const UNIDBG_LIBS = [
  { name: 'native-lib-loader', path: 'org/scijava/native-lib-loader/2.3.5/native-lib-loader-2.3.5.jar', bytes: 20352, sha256: '7db19cf3e9cfe993e725e82cd9593a8f3e2d340ed3047c98d5c54370571c759d' },
  { name: 'unidbg-android-0.9.9.jar', path: 'com/github/zhkl0228/unidbg-android/0.9.9/unidbg-android-0.9.9.jar', bytes: 6128061, sha256: 'ce52ddda7772ac79adb4606dbaf0a65c53c41af96e73299a98eadec608f270fc' },
  { name: 'unidbg-api-0.9.9.jar', path: 'com/github/zhkl0228/unidbg-api/0.9.9/unidbg-api-0.9.9.jar', bytes: 418830, sha256: '422a2d93c4f22de8436867415fc34bdacbdc62de351b828a0672651344c46efe' },
  { name: 'unidbg-unicorn2-0.9.9.jar', path: 'com/github/zhkl0228/unidbg-unicorn2/0.9.9/unidbg-unicorn2-0.9.9.jar', bytes: 6150256, sha256: '6c258e54e99da8a4e3ba4a705c5f78bcb658638ef16682ad1d3bc25953908ffb' },
  { name: 'unidbg-dynarmic-0.9.9.jar', path: 'com/github/zhkl0228/unidbg-dynarmic/0.9.9/unidbg-dynarmic-0.9.9.jar', bytes: 8217374, sha256: 'd1decada24040ae543c7cd1182c87433b4a8e9700162e7129f18937b4f3edbb9' },
  { name: 'apk-parser-2.6.10.jar', path: 'net/dongliu/apk-parser/2.6.10/apk-parser-2.6.10.jar', bytes: 213647, sha256: '4e07ef65005c0d7e7149f6e903828fdaa031f542ad3956f0d827197a5ccac640' },
  { name: 'unicorn-1.0.15.jar', path: 'com/github/zhkl0228/unicorn/1.0.15/unicorn-1.0.15.jar', bytes: 6193524, sha256: '7fbce2a0decf475b75b355cf33301349bf7c88f31cd93c7922e9dbbed3debf23' },
  { name: 'capstone-3.1.8.jar', path: 'com/github/zhkl0228/capstone/3.1.8/capstone-3.1.8.jar', bytes: 3159591, sha256: '920e5fd1d514f146891a82cfbbfb33ef20cfca746ddcd1a6e2b54fff2a2e53dc' },
  { name: 'keystone-0.9.7.jar', path: 'com/github/zhkl0228/keystone/0.9.7/keystone-0.9.7.jar', bytes: 4731494, sha256: '3db2ee22fa5b3a70800bc5dfb515acf6bcb62888be0da43c311ff8304411558b' },
  { name: 'demumble-1.0.4.jar', path: 'com/github/zhkl0228/demumble/1.0.4/demumble-1.0.4.jar', bytes: 968653, sha256: '2351bff06a0a494734924f4fa2e7084c0699d7e959a0be6e85e7d9ee17ae6eb6' },
  { name: 'jna-5.14.0.jar', path: 'net/java/dev/jna/jna/5.14.0/jna-5.14.0.jar', bytes: 1878533, sha256: '34ed1e1f27fa896bca50dbc4e99cf3732967cec387a7a0d5e3486c09673fe8c6' },
  { name: 'commons-codec-1.21.0.jar', path: 'commons-codec/commons-codec/1.21.0/commons-codec-1.21.0.jar', bytes: 402494, sha256: '4da851cb6abfb98bfe9eb77c5e5fc47f5414fa28b94e21b7fd9a646705dc167f' },
  { name: 'commons-collections4-4.5.0.jar', path: 'org/apache/commons/commons-collections4/4.5.0/commons-collections4-4.5.0.jar', bytes: 898652, sha256: '00f93263c267be201b8ae521b44a7137271b16688435340bf629db1bac0a5845' },
  { name: 'commons-io-2.21.0.jar', path: 'commons-io/commons-io/2.21.0/commons-io-2.21.0.jar', bytes: 585274, sha256: '7d643a2afea8b058b762aa6fb90e5b256f6c729739f8b3784c3370ddc609e88d' },
  { name: 'fastjson-1.2.83.jar', path: 'com/alibaba/fastjson/1.2.83/fastjson-1.2.83.jar', bytes: 671701, sha256: '641a4d65ab32fbfdccd9c718e3f83ebc4caabdb5e4fe5b3d51527c5fe692631d' },
]

async function ensureUnidbgVendor() {
  mkdirSync(UNIDBG_VENDOR, { recursive: true })
  const missing = UNIDBG_LIBS.filter((lib) => {
    const target = path.join(UNIDBG_VENDOR, lib.name)
    if (!existsSync(target)) return true
    if (statSync(target).size !== lib.bytes) return true
    return sha256File(target) !== lib.sha256
  })
  if (missing.length === 0) return
  log(`补齐 unidbg 依赖：${missing.map((l) => l.name).join('、')}`)
  for (const lib of missing) {
    const target = path.join(UNIDBG_VENDOR, lib.name)
    let done = false
    for (const base of MAVEN_BASES) {
      const url = `${base}/${lib.path}`
      try {
        const res = await fetch(url, { redirect: 'follow' })
        if (!res.ok) continue
        const buf = Buffer.from(await res.arrayBuffer())
        if (crypto.createHash('sha256').update(buf).digest('hex') !== lib.sha256) continue
        writeFileSync(target, buf)
        log(`  ${lib.name} ✓ ${Math.round(buf.length / 1024)} KB`)
        done = true
        break
      } catch { /* 换镜像 */ }
    }
    if (!done) die(`unidbg 依赖下载失败：${lib.name}`)
  }
}

const MAVEN_BASES = [
  'https://maven.aliyun.com/repository/public',
  'https://repo1.maven.org/maven2',
]

function sha256File(file) {
  return crypto.createHash('sha256').update(readFileSync(file)).digest('hex')
}

async function ensureVendor() {
  mkdirSync(VENDOR, { recursive: true })
  const missing = MAVEN_LIBS.filter((lib) => {
    const target = path.join(VENDOR, lib.name)
    if (!existsSync(target)) return true
    if (statSync(target).size !== lib.bytes) return true
    return sha256File(target) !== lib.sha256
  })
  if (missing.length === 0) return
  log(`补齐 Maven 依赖：${missing.map((l) => l.name).join('、')}`)
  for (const lib of missing) {
    const target = path.join(VENDOR, lib.name)
    let done = false
    for (const base of MAVEN_BASES) {
      const url = `${base}/${lib.path}`
      try {
        const res = await fetch(url, { redirect: 'follow' })
        if (!res.ok) {
          log(`  ${url} → HTTP ${res.status}`)
          continue
        }
        const buf = Buffer.from(await res.arrayBuffer())
        const got = crypto.createHash('sha256').update(buf).digest('hex')
        if (got !== lib.sha256) {
          console.error(`[build-jvm] ${lib.name} SHA-256 不匹配（${got}），换镜像`)
          continue
        }
        writeFileSync(target, buf)
        log(`  ${lib.name} ✓ ${Math.round(buf.length / 1024)} KB`)
        done = true
        break
      } catch (err) {
        log(`  ${url} → ${err.message}`)
      }
    }
    if (!done) die(`Maven 依赖下载失败：${lib.name}`)
  }
}

await ensureVendor()
await ensureUnidbgVendor()

const libJars = existsSync(VENDOR)
  ? readdirSync(VENDOR).filter((n) => n.endsWith('.jar')).map((n) => path.join(VENDOR, n)).sort()
  : []
if (libJars.length < MAVEN_LIBS.length) die(`jvm/vendor 里只有 ${libJars.length} 个 jar（期望 ${MAVEN_LIBS.length}）`)
const libClasspath = libJars.join(path.delimiter)


// ---- 夹具（P1 联调）：只在 --fixtures 时编译，不落 resources ----
const fixturesEnabled = argv.includes('--fixtures')
if (clean) {
  rmSync(BUILD, { recursive: true, force: true })
  if (!checkOnly) rmSync(OUT, { recursive: true, force: true })
}
mkdirSync(BUILD, { recursive: true })

// ---- 1) 编译 stubs（排除 shell-shim：那是 P4 的影子类，靠 classpath 顺序覆盖，不进 stubs.jar）----
const stubsSrc = path.join(UP, 'stubs-src')
const stubFiles = [
  ...walk(stubsSrc, (f) => f.endsWith('.java') && !/[\\/]shell-shim[\\/]/.test(f)),
  ...walk(COMPAT_SRC, (f) => f.endsWith('.java')),
]
const stubsClasses = path.join(BUILD, 'stubs-classes')
mkdirSync(stubsClasses, { recursive: true })

function compileArgfile(files, file, extra = []) {
  // javac 的 @argfile 里必须是绝对路径；Windows 下也接受正斜杠
  const lines = [...extra, ...files.map((f) => `"${f.replace(/\\/g, '/')}"`)]
  writeFileSync(file, lines.join('\n') + '\n', 'utf8')
  return file
}

// stubs 里的自研进程内 dex 转换器（tvbox.dex.DexStreamConverter）需要 dex2jar + asm 才能编译；
// 运行时这些 jar 由宿主 classpath 提供（见 crates/vbox_jvm/src/runtime.rs 的 dex2jar_jars()）。
const dex2jarVendorForStubs = path.join(root, 'jvm', 'vendor', 'dex2jar')
const stubsCompileCp = [
  libClasspath,
  ...(existsSync(dex2jarVendorForStubs)
    ? readdirSync(dex2jarVendorForStubs).filter((f) => f.endsWith('.jar')).map((f) => path.join(dex2jarVendorForStubs, f))
    : []),
].filter(Boolean).join(path.delimiter)

log(`编译 stubs：${stubFiles.length} 个源文件（上游 + 自研补件）→ stubs.jar`)
run(
  jdk.javac,
  ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-Xlint:none',
   '-cp', stubsCompileCp, '-sourcepath', stubsSrc + path.delimiter + COMPAT_SRC, '-d', stubsClasses,
   '@' + compileArgfile(stubFiles, path.join(BUILD, 'stubs-files.txt'))],
  'javac (stubs)',
)

// ---- 2) 编译宿主 ----
const hostFiles = walk(HOST_SRC, (f) => f.endsWith('.java'))
if (!hostFiles.length) die(`jvm/host/src 下没有 .java`)
const hostClasses = path.join(BUILD, 'host-classes')
mkdirSync(hostClasses, { recursive: true })
log(`编译宿主：${hostFiles.map((f) => path.basename(f)).join(', ')}`)
run(
  jdk.javac,
  ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-Xlint:none',
   '-cp', stubsClasses + path.delimiter + libClasspath, '-d', hostClasses,
   '@' + compileArgfile(hostFiles, path.join(BUILD, 'host-files.txt'))],
  'javac (host)',
)

// ---- 2.5) 编译工具 jar（DexJarRepair：dex2jar 之后修 R8 `NEW` 误译）----
const REPAIR_SRC = path.join(
  root,
  'jvm', 'upstream', 'winbox', 'native-bridge', 'com', 'winbox', 'nativebridge', 'DexJarRepair.java',
)
const REWRITER_SRC = path.join(root, 'jvm', 'upstream', 'winbox', 'tools', 'JarRewriter.java')
let toolsInfo = null
if (existsSync(REPAIR_SRC) && existsSync(REWRITER_SRC)) {
  const vendorDex2jar = path.join(root, 'jvm', 'vendor', 'dex2jar')
  const asmCp = existsSync(vendorDex2jar)
    ? readdirSync(vendorDex2jar)
        .filter((f) => f.endsWith('.jar') && (f.startsWith('asm-') || f === 'asm.jar'))
        .sort()
        .map((f) => path.join(vendorDex2jar, f))
        .join(path.delimiter)
    : ''
  const toolsClasses = path.join(BUILD, 'tools-classes')
  mkdirSync(toolsClasses, { recursive: true })
  run(
    jdk.javac,
    ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-Xlint:none',
     '-cp', asmCp, '-d', toolsClasses, REPAIR_SRC, REWRITER_SRC],
    'javac (tools)',
  )
  const toolsJar = path.join(OUT, 'jvm-tools.jar')
  run(jdk.jar, ['--create', '--file', toolsJar, '-C', toolsClasses, '.'], 'jar (jvm-tools)')
  toolsInfo = {
    bytes: statSync(toolsJar).size,
    sha256: sha256(toolsJar),
    classes: walk(toolsClasses, (f) => f.endsWith('.class')).length,
  }
  log(`jvm-tools.jar    ${(toolsInfo.bytes / 1024).toFixed(0)} KB / ${toolsInfo.classes} 类`)
}


// ---- 2.6) 编译 native-bridge.jar（ARM32 加固壳：unidbg + 我们补的 SocketReadiness）----
const unidbgJars = readdirSync(UNIDBG_VENDOR).filter((f) => f.endsWith('.jar')).map((f) => path.join(UNIDBG_VENDOR, f))
const bridgeSrcDir = path.join(root, 'jvm', 'upstream', 'winbox', 'native-bridge')
const bridgeCompatDir = path.join(root, 'jvm', 'bridge-compat')
let bridgeInfo = null
if (existsSync(bridgeSrcDir)) {
  const unidbgCp = unidbgJars.join(path.delimiter)
  const vendorDex2jar2 = path.join(root, 'jvm', 'vendor', 'dex2jar')
  const asmJar = existsSync(vendorDex2jar2)
    ? readdirSync(vendorDex2jar2).filter((f) => f.startsWith('asm-9')).sort().map((f) => path.join(vendorDex2jar2, f)).join(path.delimiter)
    : ''
  const bridgeClasses = path.join(BUILD, 'native-bridge-classes')
  mkdirSync(bridgeClasses, { recursive: true })
  const bridgeFiles = [
    ...walk(bridgeSrcDir, (f) => f.endsWith('.java')),
    ...(existsSync(bridgeCompatDir) ? walk(bridgeCompatDir, (f) => f.endsWith('.java')) : []),
  ]
  run(
    jdk.javac,
    ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-Xlint:none',
     '-cp', [unidbgCp, asmJar].filter(Boolean).join(path.delimiter), '-d', bridgeClasses,
     '@' + compileArgfile(bridgeFiles, path.join(BUILD, 'bridge-files.txt'))],
    'javac (native-bridge)',
  )
  const bridgeJar = path.join(OUT, 'native-bridge.jar')
  run(jdk.jar, ['--create', '--file', bridgeJar, '-C', bridgeClasses, '.'], 'jar (native-bridge)')
  const unidbgOut = path.join(OUT, 'unidbg')
  mkdirSync(unidbgOut, { recursive: true })
  for (const jar of unidbgJars) copyFileSync(jar, path.join(unidbgOut, path.basename(jar)))
  bridgeInfo = {
    bytes: statSync(bridgeJar).size,
    sha256: sha256(bridgeJar),
    classes: walk(bridgeClasses, (f) => f.endsWith('.class')).length,
    unidbg: unidbgJars.map((j) => ({ name: path.basename(j), bytes: statSync(j).size })),
  }
  log(`native-bridge.jar ${(bridgeInfo.bytes / 1024).toFixed(0)} KB / ${bridgeInfo.classes} 类（+ unidbg ${unidbgJars.length} jar）`)
}

// ---- 2.7) 编译 shell-shim.jar（纯 Java 影子 DexNative：加固壳解密出内层 dex 后的快路径）----
// 只在 classpath 里排在 stubs.jar **之前** 时生效（靠类加载顺序盖掉壳里的 native DexNative）。
// 由宿主按需启用（VBOX_SHELL_SHIM + VBOX_SHELL_SHIM_CLASSES），默认不进 classpath。
const shimSrcDir = path.join(stubsSrc, 'shell-shim')
let shimInfo = null
if (existsSync(shimSrcDir)) {
  const shimClasses = path.join(BUILD, 'shell-shim-classes')
  mkdirSync(shimClasses, { recursive: true })
  const shimExtra = path.join(root, 'jvm', 'shim-extra')
  const shimFiles = [
    ...walk(shimSrcDir, (f) => f.endsWith('.java')),
    ...(existsSync(shimExtra) ? walk(shimExtra, (f) => f.endsWith('.java')) : []),
  ]
  run(
    jdk.javac,
    ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-Xlint:none',
     '-cp', stubsClasses, '-d', shimClasses,
     '@' + compileArgfile(shimFiles, path.join(BUILD, 'shell-shim-files.txt'))],
    'javac (shell-shim)',
  )
  const shimJar = path.join(OUT, 'shell-shim.jar')
  run(jdk.jar, ['--create', '--file', shimJar, '-C', shimClasses, '.'], 'jar (shell-shim)')
  shimInfo = {
    bytes: statSync(shimJar).size,
    sha256: sha256(shimJar),
    classes: walk(shimClasses, (f) => f.endsWith('.class')).length,
  }
  log(`shell-shim.jar   ${(shimInfo.bytes / 1024).toFixed(0)} KB / ${shimInfo.classes} 类`)
}


// 校验目标 class 版本（61 = Java 17），否则宿主与 stubs 不可能在同一次启动里协商成功
function majorOf(file) {
  const buf = readFileSync(file)
  return buf.readUInt16BE(6)
}
const stubsSample = walk(stubsClasses, (f) => f.endsWith('.class'))[0]
const hostSample = walk(hostClasses, (f) => f.endsWith('.class'))[0]
for (const f of [stubsSample, hostSample]) {
  const v = majorOf(f)
  if (v !== 61) die(`class 版本不是 61（Java 17）：${path.relative(root, f)} = ${v}`)
}
log('class 版本校验通过（61 / Java 17）')

if (checkOnly) {
  log('--check：编译通过，跳过打包')
  process.exit(0)
}

// ---- 3) 打包 + 落 resources ----
mkdirSync(OUT, { recursive: true })
mkdirSync(path.join(OUT, 'libs'), { recursive: true })

const stubsJar = path.join(OUT, 'stubs.jar')
const hostJar = path.join(OUT, 'spider-host.jar')
run(jdk.jar, ['--create', '--file', stubsJar, '-C', stubsClasses, '.'], 'jar (stubs)')
run(jdk.jar, ['--create', '--file', hostJar, '-C', hostClasses, '.'], 'jar (host)')

// ASM 既是 jvm-tools（DexJarRepair/JarRewriter）的依赖，也是原生桥 hasNativeMethods 的依赖；
// 放进 libs 后随宿主 classpath 一起可见（体积很小，~0.5 MB）。
const libs = [...libJars]
const asmVendor = path.join(root, 'jvm', 'vendor', 'dex2jar')
if (existsSync(asmVendor)) {
  for (const f of readdirSync(asmVendor).filter((n) => n.startsWith('asm') && n.endsWith('.jar')).sort()) {
    libs.push(path.join(asmVendor, f))
  }
}

const libsOut = []
for (const jar of libs) {
  const target = path.join(OUT, 'libs', path.basename(jar))
  copyFileSync(jar, target)
  libsOut.push({ name: path.basename(jar), bytes: statSync(target).size, sha256: sha256(target) })
}
rmSync(path.join(OUT, 'libs', 'jython-standalone.jar'), { force: true })

const countClasses = (dir) => walk(dir, (f) => f.endsWith('.class')).length
const stubsInfo = { bytes: statSync(stubsJar).size, sha256: sha256(stubsJar), classes: countClasses(stubsClasses) }
const hostInfo = { bytes: statSync(hostJar).size, sha256: sha256(hostJar), classes: countClasses(hostClasses) }
// ---- dex2jar 运行时（把 dex 样本转成 class）----
const dex2jarSrc = path.join(root, 'jvm', 'vendor', 'dex2jar')
const dex2jarOut = path.join(OUT, 'dex2jar')
const dex2jarJars = existsSync(dex2jarSrc)
  ? readdirSync(dex2jarSrc).filter((f) => f.endsWith('.jar')).sort()
  : []
if (dex2jarJars.length) {
  rmSync(dex2jarOut, { recursive: true, force: true })
  mkdirSync(dex2jarOut, { recursive: true })
  for (const f of dex2jarJars) copyFileSync(path.join(dex2jarSrc, f), path.join(dex2jarOut, f))
}

const manifest = {
  rev: crypto.createHash('sha256').update(stubsInfo.sha256 + hostInfo.sha256).digest('hex').slice(0, 12),
  builtAt: new Date().toISOString(),
  release: Number(RELEASE),
  javac: versionText,
  stubs: stubsInfo,
  host: hostInfo,
  libs: libsOut,
  tools: toolsInfo,
  nativeBridge: bridgeInfo,
  shellShim: shimInfo,
}
writeFileSync(path.join(OUT, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n', 'utf8')

log(`stubs.jar        ${(stubsInfo.bytes / 1024).toFixed(0)} KB / ${stubsInfo.classes} 类`)
log(`spider-host.jar  ${(hostInfo.bytes / 1024).toFixed(0)} KB / ${hostInfo.classes} 类`)
log(`libs/            ${libsOut.length} 个 jar / ${(libsOut.reduce((s, j) => s + j.bytes, 0) / 1048576).toFixed(1)} MB`)
log(`manifest.rev     ${manifest.rev}`)

// ---- 3) 夹具（不随包分发）----
if (fixturesEnabled) {
  const fxSrc = path.join(root, 'jvm', 'fixtures', 'src')
  const fxOut = path.join(BUILD, 'fixtures-classes')
  rmSync(fxOut, { recursive: true, force: true })
  mkdirSync(fxOut, { recursive: true })
  const files = walk(fxSrc, (f) => f.endsWith('.java'))
  if (files.length === 0) die('jvm/fixtures/src 里没有 .java')
  run(jdk.javac, ['--release', RELEASE, '-encoding', 'UTF-8', '-nowarn', '-cp',
    [stubsClasses, ...libJars].join(path.delimiter), '-d', fxOut, ...files], 'javac fixtures')
  const jarPath = path.join(BUILD, 'fixtures', 'test-spider.jar')
  mkdirSync(path.dirname(jarPath), { recursive: true })
  rmSync(jarPath, { force: true })
  run(jdk.jar, ['--create', '--file', jarPath, '-C', fxOut, '.'], 'jar fixtures')
  log(`夹具: ${jarPath}`)
}
