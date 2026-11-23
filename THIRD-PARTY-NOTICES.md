# 第三方组件声明（THIRD-PARTY NOTICES）

VBox 本体以 **AGPL-3.0-only** 发布。jar 蜘蛛车道（`jvm/`、`crates/vbox_jvm`）在运行时/构建时使用下列第三方组件；
本文件按许可证要求保留其版权与许可信息。分发安装包时本文件随包附带。

## 1. Win-box 兼容层源码（MIT）

- 上游：https://github.com/yagami-hue/Win-box ，锁定 commit `80668243a783addda747d89cb4d48ab164966e30`
- 取用范围（`jvm/upstream/winbox/`）：`resources/jvm/` 下的 stubs 增量源码、`SpiderRunner.java`、`tools/JarRewriter.java`、`native-bridge/`（`DexJarRepair` 等）
- 许可：MIT License，Copyright (c) 2026 yagami-hue；完整文本见 `jvm/upstream/winbox/LICENSE`
- 我们的改动记录见 `jvm/upstream/winbox/SOURCE.txt`

## 2. 我们自研的兼容层补件（AGPL-3.0-only）

`jvm/compat/src/`（Log、Pair、DisplayMetrics、Looper、MessageQueue、SharedPreferences、Bitmap、Matrix、ValueCallback、`com.github.catvod.net.OkHttp`、Base64 等）
由本仓库自行实现，不是上游代码，随 VBox 以 AGPL-3.0-only 授权。

## 3. Maven 运行时库（随 `src-tauri/resources/jvm/libs/` 分发）

| 组件 | 版本 | 许可 |
|---|---|---|
| OkHttp | 3.14.9 | Apache-2.0 |
| Okio | 1.17.5 | Apache-2.0 |
| Gson | 2.10.1 | Apache-2.0 |
| jsoup | 1.17.2 | MIT |
| org.json | 20180813 | Public Domain / JSON License（仅用于兼容层契约） |
| SLF4J API / Simple | 2.0.16 | MIT |
| Bouncy Castle (bcprov-jdk18on) | 1.78.1 | Bouncy Castle License（MIT 风格） |
| sqlite-jdbc | 3.46.1.0 | Apache-2.0 |

## 4. dex2jar 工具链（随 `src-tauri/resources/jvm/dex2jar/` 分发）

- `dex-tools` / `dex-translator` / `dex-reader(-api)` / `dex-ir` / `dex-writer` / `d2j-smali` / `d2j-jasmin` / `d2j-base-cmd` / `d2j-external` 2.4.28，Apache-2.0
- ASM 9.7（`asm`、`asm-tree`、`asm-util`、`asm-commons`、`asm-analysis`），BSD-3-Clause
- Guava 33.2.1-jre，Apache-2.0；commons-cli 1.5.0，Apache-2.0

## 5. Java 运行时（`src-tauri/resources/jvm/jre/`）

- Eclipse Temurin JRE 17.0.21+9（Adoptium），**GPL-2.0 with Classpath Exception**（分发 JRE 需随附 GPLv2+CE 文本与 Temurin 版权信息；可在安装包许可页展示，或提供单独下载脚本 `scripts/fetch-jre.ps1` 而不随包）

> 备注：当前 `jvm/` 目录下的 jar 由 `npm run build:jvm` 本地生成，仓库只提交源码与脚本；产物不进 git。

## 6. 参考实现（未取代码）

- MisaTV（GPL-3.0）仅作行为参考，未复制其代码；如后续引入其 shell-shim，需在本文追加 GPL-3.0 声明与源码获取方式。
