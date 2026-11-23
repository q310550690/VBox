# jvm/compat/ —— VBox 自研的兼容层补件

`jvm/upstream/winbox/stubs-src/` 是上游的**增量**源码集：完整 stubs.jar 里有一部分类
（Log、Looper、Bitmap、DisplayMetrics、Pair、ValueCallback、SharedPreferences、OkHttp）
只存在于上位机构建的基线 jar 中，上游仓库里没有对应源码（其 README 明确说基线 jar 不入库）。

本目录用**净室方式**补齐这 8 个类，与 `jvm/host/src/` 一样属于 VBox 自有代码（AGPL-3.0-only），
不来自 MisaTV 的 GPL fork，也不复制任何第三方实现：

| 类 | 用途 | 完成度 |
|---|---|---|
| android.util.Log | 蜘蛛日志 → stderr | 完整 |
| android.util.Pair | Uri 查询参数 | 完整 |
| android.util.DisplayMetrics | TV 屏指标（1920×1080 / dpi 480） | 完整（固定值） |
| android.os.Looper | 提供主线程 Looper；loop() 不阻塞 | 完整（无消息队列） |
| android.webkit.ValueCallback | WebView/CookieManager 回调 | 完整 |
| android.content.SharedPreferences | 内存实现 + `Mem`（供 prefs-shim 影子覆盖） | 完整（无磁盘持久化） |
| android.graphics.Bitmap | 宽高/像素数组/缩放 | 可用；`compress()` 未编码（P1） |
| com.github.catvod.net.OkHttp | okhttp3 封装：client/dns/string/post | 可用；无缓存层 |
| android.util.LruCache | 蜘蛛常用图片/响应缓存（LinkedHashMap 访问序） | 完整 |
| android.util.Base64 | 编码常量 + encode/decode | 完整 |
| android.app.UiModeManager | TV/手机判定（桌面恒 MODE_NORMAL/NIGHT_NO） | 完整（固定值） |
| android.os.StatFs | 磁盘空间（基于 File.*Space） | 完整 |
| android.graphics.Matrix | Bitmap 变换参数（3×3 仿射） | stub；不影响解码/缩放 |
| android.os.MessageQueue | Looper 的占位队列 | stub |

## tvbox/dex/ —— 壳载荷的 dex 工具（自研）

| 类 | 用途 |
|---|---|
| `DexStreamConverter` | 进程内 dex→class（`Dex2Asm` + 自写 `ClassVisitor`/`ZipOutputStream`，绕开 JDK zipfs），reader 配置对齐上游 `Dex2jar`（`SKIP_DEBUG\|IGNORE_READ_EXCEPTION`） |
| `ConvertMain` | 独立子 JVM 入口（`java -cp "stubs.jar;dex2jar/*" tvbox.dex.ConvertMain <src> <out>`）：先宽容解 zip（壳写出的 zip 元数据不可信），再转换，stdout 打印 `CLASSES=`/`NEWFIX=` |
| `NewSiteRepair` | 修 dex2jar 对 R8「去掉构造器」类的误译：去 `ACC_ABSTRACT` 用**原字节 access_flags 补丁**（不重写代码布局，避免把加固壳混淆类跑坏），缺 `<init>` 才走 ASM 往返；`-Dtvbox.newrepair.mode=both\|ctor\|abstract\|none` 可做二分 |

> 历史教训：早期用 ASM 整类往返去 abstract，会让加固壳内层类在解释器里跑成非法状态
> （`EXCEPTION_ACCESS_VIOLATION`，`-Xint` 同样崩）。详见 `docs/type3兼容/P4-验证记录.md` §2。

`jvm/shim-extra/` 保留空目录：曾放影子 `MyCrypto`（纯 Java 假实现），实测会让壳把配置解成垃圾、
`homeContent` 返回空表，因此**已移除**，改为让 native `MyCrypto` 自然抛 `UnsatisfiedLinkError`
（统一映射 `catvod.host.shell_unsupported`）。

构建脚本（`scripts/build-jvm.mjs`）把这批源码和上游源码**一起**编进 `stubs.jar`，
所以补件对蜘蛛来说与上游基线类无区别。新增补件请同步更新本表。
