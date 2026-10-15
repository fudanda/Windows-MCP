# Windows-MCP 桌面管理器

基于 **Tauri 2 + React + Rust**，直接启动本仓库的官方 `windows-mcp` 后端。桌面操作工具仍由 CursorTouch/Windows-MCP 提供。

- 默认简体中文，可随时切换 English，重启后保留选择。
- 启动、停止后端，退出应用时回收本应用启动的整个进程树。
- 默认仅监听 `127.0.0.1`，开启局域网后监听 `0.0.0.0`。
- 可配置端口和 Token，生成新 Token，复制客户端 TOML / JSON 配置。
- Token 使用当前 Windows 用户的 DPAPI 加密保存，通过环境变量传给后端，不出现在启动参数或明文配置文件中。
- 检测连接会实际执行 MCP `initialize` 和 `tools/list`，完成后释放会话。
- 运行日志最多保留 200 行，并隐藏 Token。

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
npm run tauri dev
```

开发模式自动使用仓库 `.venv`；普通浏览器只能预览界面，不能启动本机进程或保存管理器配置。

## 构建桌面安装包

```powershell
cd desktop
npm ci
npm run build:desktop
```

此命令先用 PyInstaller 打包官方 Python 后端，再构建 Tauri / NSIS 安装程序。安装后不依赖目标电脑上的 Python、uv 或源码目录。需要 WebView2，安装程序使用 Tauri 默认的 WebView2 安装机制。

生成位置：`src-tauri/target/release/bundle/nsis/`。

## 使用

1. 默认生成随机 Token，选择监听端口。
2. 如果要从另一台电脑连接，开启“允许局域网连接”。
3. 点击“保存配置”或“保存并启动”。
4. 在客户端连接区域选择本机或局域网 IP，复制配置到 MCP 客户端。
5. 使用“检测连接”确认协议握手和工具发现成功。

局域网使用可信网络中的 HTTP + Bearer Token；应用不会自动修改 Windows 防火墙。需要远程设备连接时，请放行所选端口。服务运行期间连接设置只读；停止后再修改，语言切换不受影响。

配置位于 `%APPDATA%/com.fudanda.windowsmcp.manager/manager.json`。其中 Token 为 DPAPI 密文，仅能由原 Windows 用户解密。管理器使用独立、不含 Token 的 `backend.toml`，不覆盖 `~/.windows-mcp/config.toml`。匿名遥测在此管理器启动的后端中默认关闭；保留官方桌面控制保护机制。

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

Run `uv sync --locked --python 3.14` in the repository root, then `npm ci` and `npm run tauri dev` in `desktop`. To build a Windows installer with the bundled Python backend, run `npm run build:desktop`. Exiting the manager terminates only the backend process tree it owns. Existing user-level Windows-MCP configuration remains separate.
