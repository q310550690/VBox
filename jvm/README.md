# jvm/ —— jar 蜘蛛的 JVM 车道

目标：在本地真 JRE 17 里跑**原始 jar 字节码**，由净室重写的 Android/TVBox 兼容层（stubs）提供运行环境。
设计见 `docs/type3兼容/type3-JVM兼容层实施计划.md`，P0 验收见 `docs/type3兼容/P0-验证记录.md`。

```
jvm/
├─ upstream/winbox/         上游 MIT 源码（原样 vendor，见 SOURCE.txt）
│   ├─ SpiderRunner.java      宿主原版（--serve/--hold/一次性）
│   ├─ stubs-src/             Android/TVBox 兼容层源码（增量集，缺 8 类）
│   ├─ tools/JarRewriter.java  ASM 重算栈映射帧（P4）
│   └─ native-bridge/         unidbg 桥（P4/P5）
├─ compat/src/              VBox 自研补件（AGPL-3.0-only，10 个类，补齐 stubs-src 缺口）
├─ host/src/SpiderHost.java VBox 的宿主 fork（+ --probe）
├─ vendor/                  Maven 依赖（构建脚本自动下载，不入库）
└─ build/                   编译中间产物（不入库）
```

## 构建

```bash
node scripts/build-jvm.mjs        # 或 npm run build:jvm
```

会做四件事：缺 Maven 依赖就下载（固定 sha256）→ `javac --release 17` 编译 stubs（上游 + `compat/src`）
与宿主 → 校验 class 版本 61 → 产出到 `src-tauri/resources/jvm/`：

| 产物 | 说明 |
|---|---|
| `stubs.jar` | 257 类，Android/TVBox 兼容层 |
| `spider-host.jar` | 7 类，宿主（`SpiderHost`） |
| `libs/*.jar` | 9 个运行时依赖（okhttp/gson/jsoup/json/sqlite/slf4j/bc） |
| `manifest.json` | 产物清单：rev、class 数、每个 jar 的 bytes + sha256 |

需要 JDK 17+（`VBOX_JDK_HOME` / `JAVA_HOME` / PATH 任一；用 21 的 javac 配 `--release 17` 也可以）。
`--check` 只编译不落盘，`--clean` 先清中间目录。

## 运行时供给（JRE）

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-jre.ps1           # 下载并 stage
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-jre.ps1 -Verify    # 只校验
bash scripts/fetch-jre.sh --verify                                                  # 非 Windows 等价路径
```

固定 Temurin **17.0.21+9** JRE（zip 43,860,024 B / sha256 `fe4656bd…d9d117`，与 Adoptium 官方元数据一致），
解压压平到 `src-tauri/resources/jvm/jre/`（316 文件 / 125 MB），并写 `marker.json`。
镜像顺序：清华 TUNA → api.adoptium.net → GitHub release。

## 自检

```bash
cargo run -p vbox_cli -- jvm-ping --runs 5     # 装配 + 冷启动（P0 实测中位 151 ms）
cargo test -p vbox_jvm                        # 7 个单测（含真实运行时探针）
```

## 打包

`scripts/ensure-tools.mjs` 的 `TOOLS.jvm` 会先跑 `build-jvm.mjs`，再跑 `fetch-jre.ps1`；
`tauri.conf.json` 的 `bundle.resources` 已含 `resources/jvm/`。不想内置时用
`npm run tauri:build -- --bundle ffmpeg,python,node`。
