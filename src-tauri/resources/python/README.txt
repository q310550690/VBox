此目录用于「随安装包集成」的 Python（嵌入式 CPython + 常用 spider 依赖）。

发布构建前执行（或交给 `npm run tauri:build` 自动完成）：

    pwsh -File scripts/fetch-python.ps1

脚本会下载 Python 3.12.8 embeddable、打开 site、装好 pip 与
requests / beautifulsoup4 / lxml / pycryptodome / pyquery，
`tauri.conf.json` 的 bundle.resources 会把整个目录打进安装包。
用户不需要自己装 Python。

没有内置时，应用仍可用「设置 → Python 运行时」探测系统 Python 或手动指定路径。

注意：这里的内容体积大且可重建，不进 git（见根目录 .gitignore）。
