此目录用于「随安装包集成」的 FFmpeg。

发布构建前执行：

    pwsh -File scripts/fetch-ffmpeg.ps1

脚本会把 Windows x64 静态构建的 ffmpeg.exe 放到这里，
`tauri.conf.json` 的 bundle.resources 会把整个目录打进安装包，
用户安装后无需再自己下载 FFmpeg（设置页会显示「随应用集成」）。

没有放 ffmpeg.exe 时也能打包：应用内「设置 → 外部工具 · FFmpeg →
一键下载并集成」会把固定版本下载到用户数据目录（SHA-256 校验），
效果相同，只是需要一次联网下载。

注意：ffmpeg.exe 体积很大，不进 git（见根目录 .gitignore）。
