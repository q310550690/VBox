此目录用于「随安装包集成」的 Node.js（drpy / drpy2 运行时）。

发布构建前执行（或交给 `npm run tauri:build` 自动完成）：

    pwsh -File scripts/fetch-node.ps1

脚本只从官方 zip 里取出 node.exe（约 80 MB，附 LICENSE），
`tauri.conf.json` 的 bundle.resources 会把整个目录打进安装包。
用户不需要自己装 Node。

没有内置时，应用仍可用「设置 → Node 与 drpy」探测系统 Node 或手动指定路径。

注意：这里的内容体积大且可重建，不进 git（见根目录 .gitignore）。
