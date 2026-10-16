# 在其他项目中使用 Windows-MCP

远程电脑运行 Windows-MCP Manager；操作端只需配置 MCP，或安装本仓库的 `windows-mcp` Skill。上传、下载文件和文件夹时，操作端无需安装桌面管理器，也不用浏览器选择文件。

本指南对应本项目 Manager 0.2.9。桌面操作也可连接独立 Windows-MCP 后端；文件传输需要管理器提供的 `FileTransfer` 工具。

## 1. 准备远程电脑

在管理器中启用“允许局域网连接”，设置 MCP Token，并点击“一键启动 MCP 和文件传输”。记录管理器显示的 MCP 地址，例如 `http://<设备IP>:8001/mcp`。操作端需要能访问对应端口；文件端口通常为 8002，以实际配置为准。

MCP 和文件服务使用独立 Token。Skill 助手通过 MCP 获取文件连接信息，不需要把两套 Token 手工复制到每个项目。已有运行中的服务和连接配置可直接复用。

## 2. 安装可复用 Skill

用户级安装后，同一台电脑的其他项目都能使用。在 PowerShell 中执行以下命令，将源码目录替换为实际位置：

```powershell
$source = 'D:\AI\Windows-MCP\skills\windows-mcp'
$destination = Join-Path $env:USERPROFILE '.agents\skills\windows-mcp'
if (Test-Path -LiteralPath $destination) {
    throw "目标已存在，请先比较内容再更新：$destination"
}
New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
Copy-Item -LiteralPath $source -Destination $destination -Recurse
```

也可解压分发包，将其中 `windows-mcp` 文件夹放到 `~/.agents/skills/`。若只供一个仓库使用，放到该仓库的 `.agents/skills/windows-mcp/` 即可；选择一种安装位置，避免重复安装同名 Skill。

Codex 通常会自动发现新 Skill。若当前对话没有显示，重新打开对话或重启 Codex。在其他项目中可以直接输入：

```text
$windows-mcp 连接我已配置的远程电脑，读取显示器信息并截图。
$windows-mcp 将当前项目的 dist 文件夹上传到远程 releases 目录，报告进度和 SHA-256 校验结果。
$windows-mcp 将共享目录的 reports 下载到当前项目的 received-reports 文件夹。
```

Skill 提供操作方法，不内置设备地址或凭据。首次使用仍需提供目标设备和连接授权。

## 3. 配置 Codex 的 MCP 连接

已有可用连接时跳过本节。编辑用户配置 `~/.codex/config.toml`，或受信任项目的 `.codex/config.toml`。先检查已有的 `windows-mcp` 配置项，更新同一项，避免重复的 TOML 表。

```toml
[mcp_servers.windows-mcp]
url = "http://<设备IP>:8001/mcp"
bearer_token_env_var = "WINDOWS_MCP_TOKEN"
startup_timeout_sec = 120
tool_timeout_sec = 120
```

将 `<设备IP>` 替换为管理器显示的局域网 IP。`WINDOWS_MCP_TOKEN` 放原始 Token，不带 `Bearer ` 前缀。可以用隐藏输入设置当前 PowerShell 进程的环境变量：

```powershell
$env:WINDOWS_MCP_TOKEN = [Net.NetworkCredential]::new('', (Read-Host 'MCP Token' -AsSecureString)).Password
```

Codex 必须从带有该变量的进程启动，才能读取它；在另一个终端设置变量不会更新已经运行的 Codex。也可继续使用管理器生成的现有连接配置。不要把真实 Token 提交到项目或文档。

Skill 优先使用当前会话中已加载的 MCP 工具，并发现实际参数。`Screenshot`、`DisplayInventory`、`FileTransfer` 等名称以服务端返回为准。

## 4. 没有原生 MCP 工具入口时

Skill 自带便携助手，使用 MCP 官方 Python SDK。操作端只需安装 `uv`；uv 会隔离解析 Python 3.10+ 与脚本依赖，不需要安装 Windows 后端。

```powershell
$client = Join-Path $env:USERPROFILE '.agents\skills\windows-mcp\scripts\windows_mcp_client.py'
$env:WINDOWS_MCP_URL = 'http://<设备IP>:8001/mcp'

# 已设置 WINDOWS_MCP_TOKEN 时直接使用；否则在交互终端隐藏输入 Token。
uv run --no-project --script $client tools
uv run --no-project --script $client call DisplayInventory
uv run --no-project --script $client --output-dir './artifacts/remote' call Screenshot --args-json '{"use_annotation":false}'

# 上传保留源名称，结果为远程 releases/dist。
uv run --no-project --script $client transfer -- --json upload './dist' --to 'releases'

# 下载目标是最终文件夹路径，必须不存在。
uv run --no-project --script $client transfer -- --json download 'reports' --to './received-reports'

uv run --no-project --script $client transfer -- --json list
uv run --no-project --script $client transfer -- --json status
```

这些命令可以从任意项目目录执行；相对路径始终相对于调用者的当前工作目录。路径含空格时保留引号。复杂工具参数可保存为 UTF-8 JSON 文件并使用 `call <工具> --args-file './arguments.json'`，避免终端引号兼容问题。

自动化可通过子进程 stdin 配合助手的 `--token-stdin` 传递凭据；不要将 Token 放进命令行参数。图片写为本地文件，结果 JSON 返回绝对路径，不输出 Base64。

MCP SSE 消息上限在助手中默认为 1 GiB，可通过 `--max-sse-event-size` 调整；它不修改其他客户端的设置。`--timeout` 默认 120 秒，控制助手的 MCP 和脚本引导 HTTP 请求；传输 CLI 的超时选项放在 `transfer --` 后面。

## 5. 文件传输的实际行为

助手先检查文件服务，再通过 `FileTransfer.connection` 取得文件端口、独立 Token 和客户端脚本摘要。它使用所选 MCP 主机连接文件服务，下载现有 `windows-mcp-transfer.py`，核对 SHA-256 后从临时目录运行。文件直接通过 HTTP 传输；不经 MCP 编码，也不把 Token 写入项目。

若服务未运行，在已授权的传输任务中调用 `FileTransfer(action="start")`，保留已保存配置，再执行传输。助手不会自动修改配置或启动服务。

`--json` 输出 JSON Lines 进度事件，适合 Codex 逐行读取；去掉它可显示默认简体中文的终端进度条，传入 `--language en` 可切换英文。判断成功须同时检查退出码、最终 `sha256Verified` 和任务状态。目录保留空文件、嵌套空目录和中文名称；同名目标不会自动覆盖，首版不支持断点续传或目录合并。

特别注意路径所属电脑：

| 参数 | 路径属于哪里 |
| --- | --- |
| CLI `upload './dist'` | Codex 操作端的当前项目 |
| CLI `download 'reports'` | 远程共享目录内的相对路径 |
| CLI `download … --to './received'` | Codex 操作端的最终目标路径 |
| MCP `FileTransfer.send.paths` | 运行 MCP 服务的电脑上的绝对路径，用于发送给已配对设备 |

`send.paths` 不能代替上传操作端本机文件。更多参数、进度事件、ZIP 空目录与排错说明见 Skill 的[文件传输参考](../skills/windows-mcp/references/file-transfer.md)；桌面坐标、窗口切换、中文输入见[桌面操作参考](../skills/windows-mcp/references/desktop.md)。

HTTP 传输适用于可信局域网；SHA-256 验证完整性，连接仍采用实际服务提供的传输协议。

## 参考

- [本仓库 Skill 源文件](../skills/windows-mcp/SKILL.md)
- [桌面管理器与构建说明](../desktop/README.md)
- [Codex 官方 Skill 文档](https://learn.chatgpt.com/docs/build-skills)
- [Codex 官方 MCP 配置文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
