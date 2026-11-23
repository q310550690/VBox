JVM 车道运行时（jar 蜘蛛用）——这里的内容不随仓库分发，打包前由脚本生成。

生成方式：
  node scripts/build-jvm.mjs                                  # stubs.jar / spider-host.jar / libs/
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-jre.ps1   # jre/

产物：
  jre/              Temurin JRE 17（约 126 MB）
  stubs.jar         Android/TVBox 兼容层（257 类）
  spider-host.jar   宿主 SpiderHost
  libs/*.jar        9 个运行时依赖（okhttp/gson/jsoup/json/sqlite/slf4j/bc）
  manifest.json     构建清单（入库，用于校验）
  marker.json       JRE staging 标记（机器相关，不入库）

校验：cargo run -p vbox_cli -- jvm-ping --runs 5
说明：docs/type3兼容/P0-验证记录.md
