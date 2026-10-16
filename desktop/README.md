# Windows-MCP 桌面管理器

基于 **Tauri 2 + React + Rust**，直接启动本仓库的官方 `windows-mcp` 后端。桌面操作工具仍由 CursorTouch/Windows-MCP 提供。

- 默认简体中文，可随时切换 English，重启后保留选择。
- 启动、停止后端，退出应用时回收本应用启动的整个进程树。
- “一键启动 MCP 和文件传输”会保存 MCP 设置、确认 MCP 就绪，再按已保存的文件传输配置开启文件服务；已经运行的服务会保留，部分失败时可直接重试。
- “一键停止 MCP 和文件传输”会停止两项服务并取消活动传输；启动过程中也可点击停止，等待启动请求结束后确保两项服务都已关闭。单项停止失败不会阻止另一项停止。
- 系统托盘常驻：关闭窗口后服务继续运行，左键单击 / 双击图标恢复窗口；右键菜单可显示窗口、一键启动、一键停止或退出，菜单随界面语言切换。
- 默认允许局域网连接，监听 `0.0.0.0`；连接地址优先选择已连接的以太网 / Wi-Fi 网卡 IP。手动关闭局域网后仅监听 `127.0.0.1`。
- 可配置端口和 Token，生成新 Token，复制客户端 TOML / JSON 配置。
- Token 使用当前 Windows 用户的 DPAPI 加密保存，通过环境变量传给后端，不出现在启动参数或明文配置文件中。
- 检测连接会实际执行 MCP `initialize` 和 `tools/list`，完成后释放会话。
- 运行日志最多保留 200 行，并隐藏 Token。
- 自带网页文件服务：本机只需浏览器，支持文件、文件夹上传，文件下载和文件夹 ZIP 下载。
- 传输使用独立 Token，Uppy 显示进度，收到的文件经过流式 SHA-256 和 Dufs 落盘 SHA-256 双重校验后发布。
- `FileTransfer` MCP 工具与桌面端共用配置、设备和传输任务。

## 官方参数核对

实现以本仓库的 `src/windows_mcp/__main__.py`（当前后端版本 0.8.8）和实际 `serve --help` 为准：

| 用途 | 官方参数 / 协议 |
| --- | --- |
| 启动 | `python -m windows_mcp serve --transport streamable-http` |
| 本机 / 局域网 | `--host 127.0.0.1` / `--host 0.0.0.0`，端口由 `--port` 指定 |
| 认证 | `WINDOWS_MCP_AUTH_KEY` 环境变量；客户端发送 `Authorization: Bearer <Token>` |
| 独立配置 | `--config <backend.toml>` |
| 桌面控制保护 | `--control` |
| MCP 地址 | `http://<IP>:<端口>/mcp`，使用无尾斜杠地址避免客户端重定向丢失认证头 |

官方也提供 `--auth-key`，管理器选择环境变量传递，避免在进程命令行暴露 Token。管理器默认要求 32–256 位可打印 ASCII Token，这是管理器的校验规则。

## 本地开发

需要 Windows、Python 3.14、uv、Node.js、Rust 和 Tauri 2 的 Windows 构建环境。

```powershell
# 仓库根目录
uv sync --locked --python 3.14
cd desktop
npm ci
npm run build:file-server
npm run build
npm run tauri dev
```

开发模式优先使用已打包的后端，未准备打包后端时使用仓库 `.venv`；普通浏览器只能预览管理界面，不能启动本机进程或保存管理器配置。

## 构建桌面安装包

```powershell
cd desktop
npm ci
npm run build:desktop
```

此命令先用 PyInstaller 打包官方 Python 后端，再下载并验证固定版本 Dufs，最后构建 Tauri / NSIS 安装程序。安装后不依赖目标电脑上的 Python、uv、Dufs 或源码目录。需要 WebView2，安装程序使用 Tauri 默认的 WebView2 安装机制。

`Dufs 0.46.0` 官方 Windows 压缩包 SHA-256 固定为 `21d829653ac1f178b124ed384e2a57df19b99ae404935cba9a1a5719c199b0bf`。相关组件及许可证见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

生成位置：`src-tauri/target/release/bundle/nsis/`。

## 使用

0.2.8 使用统一的 Windows-MCP 图标，覆盖安装包、卸载程序、应用窗口、桌面快捷方式、系统托盘及管理页面。设计源文件与生成提示词保存在 `artwork/`；平台图标由 Tauri CLI 导出。

1. 默认生成随机 Token，选择监听端口。
2. “允许局域网连接”默认开启；如果只需本机访问，可以手动关闭。
3. 点击“一键启动 MCP 和文件传输”同时开启两项服务；也可单独“保存配置”或“保存并启动”MCP。
4. 在客户端连接区域选择本机或局域网 IP，复制配置到 MCP 客户端。
5. 使用“检测连接”确认协议握手和工具发现成功。

局域网使用可信网络中的 HTTP + Bearer Token；应用不会自动修改 Windows 防火墙。需要远程设备连接时，请放行所选端口。服务运行期间连接设置只读；停止后再修改，语言切换不受影响。

配置位于 `%APPDATA%/com.fudanda.windowsmcp.manager/manager.json`。其中 Token 为 DPAPI 密文，仅能由原 Windows 用户解密。管理器使用独立、不含 Token 的 `backend.toml`，不覆盖 `~/.windows-mcp/config.toml`。匿名遥测在此管理器启动的后端中默认关闭；保留官方桌面控制保护机制。

## 系统托盘

- 关闭主窗口会隐藏到托盘，MCP 和文件传输继续运行；最小化仍使用 Windows 任务栏。
- 左键单击 / 双击托盘图标、选择“显示主窗口”，或再次启动应用，都会恢复已有窗口。
- 右键托盘图标可使用“一键启动 MCP 和文件传输”“一键停止 MCP 和文件传输”和“退出（停止所有服务）”。应用右上角的“托盘菜单”打开同一个菜单。
- 托盘启停使用已保存的配置；窗口按钮和托盘菜单共用 Rust 服务管理，停止或退出会取消排队中的启动，避免隐藏窗口后又启动服务。
- “退出”会回收本应用启动的 MCP、文件服务和活动传输。仅关闭窗口不会退出应用。菜单语言随简体中文 / English 设置切换。

## 局域网文件传输

0.2.7 修复 ZIP 导出遗漏空目录的问题。ZIP 保留嵌套目录、空文件及中文名称；导出完全空的文件夹时，ZIP 包含该文件夹的同名目录项。打包采用流式压缩，支持进度和取消，排除私有暂存目录及链接。

0.2.6 修复文件已完整下载却显示“已取消”的任务状态问题，覆盖普通文件、空文件和范围下载；中途断开或主动取消仍显示“已取消”。

1. 在测试机安装并打开管理器，进入“文件传输”。
2. 选择专用的“接收与共享目录”，默认是下载目录下的 `Windows-MCP`；该目录中的已发布文件可被持有传输 Token 的用户下载。
3. 默认传输端口为 `8002`，允许局域网访问；可在服务管理页点击“一键启动 MCP 和文件传输”，或在文件传输页单独点击“开启文件服务”。组合启动使用本页已保存的配置。
4. 复制网页地址和独立的传输 Token，在开发机浏览器打开 `http://测试机IP:8002`，输入 Token 连接。
5. 用 Uppy 上传文件，或用“上传文件夹”保留相对路径。点击文件下载按钮，或下载整个文件夹的 ZIP。浏览器下载列表也提供下载进度。

本机无需安装第二份桌面端。界面默认简体中文，支持 English；浏览器单独保留语言选择。完整目录中的空文件会保留；浏览器文件夹选择 API 不提供空目录，空目录可通过桌面端或 MCP 发起的目录传输保留。

上传按流处理，不把整个文件读入内存；传输服务独立于 MCP SSE 上限，首版单文件限额为 1 TiB，最多四个活动任务。校验失败或取消的文件不会发布，已存在的同名目标不会被覆盖。文件夹按文件逐个发布，任务取消时已完成的文件会保留。任务历史仅保留当前应用会话最近 100 条。

基于 Dufs 的 HTTP/WebDAV 文件 I/O、async-deflate-zip 的流式 ZIP 压缩、Uppy 的选择/队列/进度/取消，以及 Noble Hashes 的浏览器增量 SHA-256；项目负责鉴权、目录范围、发布规则、任务和 MCP 适配。ZIP 每次最多包含 10,000 个条目及 1 TiB 的源文件，压缩内容使用有界缓冲区传输，不在内存或磁盘中暂存整个压缩包。独立传输 Token 和已保存设备凭据整体使用 DPAPI 加密保存在 `transfer.json`。网页会话使用 HttpOnly、SameSite=Strict Cookie，页面和文件接口同源。HTTP 适用于可信局域网；跨设备连接需要相应端口可通过 Windows 防火墙。

## Codex 命令传输（无需浏览器或本机桌面端）

远程电脑运行管理器并开启文件服务即可。本机只需独立脚本 `scripts/windows-mcp-transfer.py`，不导入 Windows-MCP、Tauri 或 Windows API；支持 Windows、macOS 和 Linux。需要 Python 3.10+ 及 HTTPX / Rich，推荐用 `uv run --script` 自动准备隔离环境。本机无需安装或运行桌面管理器，也无需克隆整个仓库。

如果由 Codex 操作，先调用远程 MCP `FileTransfer({"action":"start"})`，再调用 `connection`。返回值包括文件服务端口、独立 Token、`client.path` 和脚本 SHA-256。Codex 在**自己的执行机器**下载脚本、核对 SHA-256，设置进程环境变量后执行上传/下载命令。文件字节直接流经 HTTP，不写入 MCP 消息或模型上下文。

PowerShell 示例（Token 通过环境变量传递，勿放进命令行参数或提交到仓库）：

```powershell
$env:WINDOWS_MCP_TRANSFER_URL = 'http://192.168.1.20:8002'
$env:WINDOWS_MCP_TRANSFER_TOKEN = '<文件传输Token>'
$transferClientUrl = $env:WINDOWS_MCP_TRANSFER_URL + '/client/windows-mcp-transfer.py'
curl.exe --fail --output windows-mcp-transfer.py $transferClientUrl
# 核对该文件的 SHA-256 与 MCP connection.client.sha256 一致后执行
Get-FileHash .\windows-mcp-transfer.py -Algorithm SHA256

# 上传文件或整个文件夹：自动保留源名称；--to 表示远程父目录
uv run --script .\windows-mcp-transfer.py --json upload 'D:\build\app' --to 'releases'
uv run --script .\windows-mcp-transfer.py --json upload 'D:\build\setup.exe'

# 下载文件或文件夹：--to 是本机最终目标路径，必须不存在
uv run --script .\windows-mcp-transfer.py --json download 'releases/app' --to 'D:\received\app'
uv run --script .\windows-mcp-transfer.py --json download 'setup.exe' --to 'D:\received\setup.exe'

uv run --script .\windows-mcp-transfer.py --json list 'releases'
uv run --script .\windows-mcp-transfer.py --json status
uv run --script .\windows-mcp-transfer.py --json cancel '<任务ID>'
```

环境变量也可在 macOS/Linux 的 shell 设置；命令和远程路径语义相同。`--url` 可覆盖地址，凭据也支持 `--token-stdin` / `--token-file`。不提供会暴露进程参数的 `--token` 选项。`--language en` 可切换英文。

默认终端进度由 Rich 显示，非交互终端输出文本进度；`--json` 输出 JSON Lines，包含 `plan`、`progress`（阶段、已传/总字节）、`verified`（每个文件的 SHA-256 和上传任务 ID）、`completed` 或 `error`。成功退出码为 0，失败为 1，参数错误为 2，Ctrl+C 取消为 130。可以从另一条 Codex 命令查询或取消服务端任务。

上传使用服务端现有的双重 SHA-256 校验；下载先读取远程 SHA-256，流式写入本机临时文件，核对大小和摘要后才发布。文件夹递归传输，保留空文件和空目录，不使用 ZIP 解包。拒绝符号链接、junction 和不安全路径；不会覆盖同名文件，也不会合并到已有的目标文件夹。每次传输最多 10,000 个条目和 1 TiB，总量不读入内存。文件夹逐文件发布，中断后已校验完成的文件会保留；首版不提供自动续传。

独立命令的回归测试不加载 Windows 桌面后端：

```powershell
uv run --no-project --python 3.14 --with httpx==0.28.1 --with rich==14.3.4 --with pytest python -m pytest -c scripts/tests scripts/tests/test_transfer_cli.py -q
```

## MCP 文件传输

由管理器启动的后端会额外注册 `FileTransfer`，独立启动原 Python 后端时不会注册此工具。MCP 只传任务信息，文件内容直接走 HTTP。示例工具参数：

```json
{"action":"status"}
{"action":"connection"}
{"action":"start"}
{"action":"add_peer","peer":{"name":"测试机","url":"http://192.168.1.20:8002","token":"<对方传输Token>"}}
{"action":"send","peer_id":"<status返回的设备ID>","paths":["D:\\build\\win-unpacked"]}
{"action":"cancel","task_id":"<send返回的任务ID>"}
```

支持 `status`、`connection`、`configure`、`start`、`stop`、`add_peer`、`remove_peer`、`send`、`cancel`。`status` 隐藏所有 Token；只有显式 `connection` 才返回本机传输 Token。`send.paths` 始终是 **MCP 服务所在电脑**的路径；Codex 本机路径应使用上一节的独立命令。两台管理器也可以互相作为接收端；普通浏览器保留为手动入口。`FileTransfer` 独立于桌面鼠标键盘控制锁运行，用户操作电脑时也能后台传输。停止 MCP 服务不会停止文件服务，退出管理器会回收其启动的后端和 Dufs 进程。

## 验证

```powershell
npm test
# 先在管理器启动服务，实际调用 DisplayInventory 和 Screenshot
npm run test:mcp
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --lib
# 显式运行真实后端的启动、认证、工具发现及停止集成验证
cargo test --manifest-path src-tauri/Cargo.toml --lib official_backend_lifecycle_and_authentication -- --ignored
```

MCP 测试客户端的单条 SSE 消息默认上限为 **1 GiB（1,073,741,824 字节）**，截图及报告保存在 `desktop/test-results/`。可用 `npm run test:mcp -- --max-sse-event-size <字节数>` 覆盖，或用 `--output-dir <目录>` 指定输出位置。Token 从管理器配置中通过当前用户的 DPAPI 解密读取，不输出到终端。

这是项目测试客户端的接收上限。服务端没有 SDK 的 1 MiB 接收限制，也不能替第三方 MCP 客户端调整各自的上限。

## English

Tauri 2 desktop manager for the official Windows-MCP backend. Defaults to Simplified Chinese with persistent English switching. Supports process start/stop, LAN binding, ports, DPAPI-encrypted bearer tokens, client configuration export, logs, and a real MCP connection check.

Version 0.2.7 preserves directory entries in ZIP exports, including nested empty directories. An entirely empty selection produces a ZIP containing the selected folder's directory entry. Archives stream through bounded buffers with progress and cancellation; private staging and links are excluded.

Version 0.2.6 fixes fully downloaded files being marked as cancelled, including empty files and range requests. Interrupted and explicitly cancelled downloads remain cancelled.

Version 0.2.5 adds a native system tray. Closing the main window hides it and keeps services running. Left-click the tray icon or launch the app again to restore the existing window. Right-click the icon, or use the window's Tray menu button, to show the window, start/stop both services using saved settings, or quit and stop all owned services. Menu labels follow the interface language. Window and tray actions share the Rust lifecycle controller so stopping also cancels queued starts.

Version 0.2.1 adds a standalone Codex transfer command. Call MCP `FileTransfer` with `start` and `connection`, fetch `client.path` from the file-service origin, verify the script against `client.sha256`, and run it locally with `uv run --script windows-mcp-transfer.py --json upload <LOCAL_PATH>` or `download <REMOTE_PATH> --to <LOCAL_DESTINATION>`. Set `WINDOWS_MCP_TRANSFER_URL` and `WINDOWS_MCP_TRANSFER_TOKEN` in the command process environment, or use `--token-stdin`. HTTPX streams the bytes, Rich displays terminal progress, and JSON Lines supports agents. Both directions verify SHA-256 and preserve empty files/directories. The client does not import or require Windows-MCP or Tauri. Only the receiving computer needs the desktop manager; a browser is optional. The MCP `send` action continues to read paths on the server, never client-local paths.

Version 0.2 adds browser-based LAN transfers using bundled Dufs, Uppy and incremental SHA-256. Start the file service on the receiving PC, then open its `http://LAN-IP:8002` address in any other PC's browser and enter the dedicated transfer token. Upload files/folders, download files or folder ZIPs, and view task progress without installing a sender application. The `FileTransfer` MCP tool shares the same Rust configuration and task engine. Existing destinations are preserved; uploads are verified before publication. Transfers use trusted-LAN HTTP; browser sessions use HttpOnly SameSite cookies. Run `npm run build:file-server` and `npm run build` once before development so the pinned Dufs executable and embedded browser assets are available.

Run `uv sync --locked --python 3.14` in the repository root, then `npm ci` and `npm run tauri dev` in `desktop`. To build a Windows installer with the bundled Python backend, run `npm run build:desktop`. Exiting the manager terminates only the backend process tree it owns. Existing user-level Windows-MCP configuration remains separate.
