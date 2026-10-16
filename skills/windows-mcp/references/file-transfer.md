# 文件和文件夹双向传输

客户端本机只需 `uv`；Python 3.10+ 和脚本依赖由 uv 隔离解析。无需在客户端安装 Windows-MCP Manager、Dufs 或浏览器上传组件。接收与共享的一端运行本项目管理器并启用文件服务。

## 路径和 Token

| 场景 | 来源 | 目标 | 方法 |
| --- | --- | --- | --- |
| 当前项目上传到远程电脑 | 客户端本机路径，如 `./dist` | 远程共享目录内的父目录，如 `releases` | 独立 CLI `upload` |
| 远程电脑下载到当前项目 | 远程共享目录内相对路径，如 `releases/dist` | 客户端最终路径，如 `./received-dist` | 独立 CLI `download` |
| 远程电脑主动发送到另一台接收端 | MCP 服务端绝对路径，如 `D:\build\app` | 已配对设备的共享目录 | `FileTransfer.send` |

MCP 地址通常为 `http://设备IP:8001/mcp`；文件服务通常为 `http://设备IP:8002`，实际端口从 `FileTransfer.connection.port` 读取。两者 Token 独立。连接信息中的 loopback 地址属于远程服务器，不能把远程 `127.0.0.1` 当成客户端本机；助手使用 MCP URL 的主机与返回的文件端口组合地址。

## 推荐：通过 Skill 助手

先调用 `FileTransfer` 的 `status`。若文件服务停止，为已授权的传输调用 `start`；不修改保存的目录、Token 或端口。默认接口为：

```json
{"action":"status"}
{"action":"start"}
```

然后在**客户端本机**运行：

```powershell
$client = '<skill-dir>/scripts/windows_mcp_client.py'

# 上传保留源名称，结果为 releases/dist；省略 --to 就放在共享目录下。
uv run --no-project --script $client transfer -- --json upload './dist' --to 'releases'
uv run --no-project --script $client transfer -- --json upload './setup.exe'

# 下载 --to 是最终路径，必须不存在；文件夹不会合并到已有目录。
uv run --no-project --script $client transfer -- --json download 'releases/dist' --to './received-dist'
uv run --no-project --script $client transfer -- --json download 'setup.exe' --to './received/setup.exe'

uv run --no-project --script $client transfer -- --json list 'releases'
uv run --no-project --script $client transfer -- --json status
uv run --no-project --script $client transfer -- --json cancel '<task-id>'

# 人工终端去掉 --json 显示 Rich 进度条，默认简体中文；--language en 切换英文。
uv run --no-project --script $client transfer -- download 'setup.exe' --to './received-2.exe'
```

助手获取 `FileTransfer.connection`，从同一服务器的文件服务下载 `client.path`，核对 `client.sha256` 后才执行。客户端脚本保存在临时目录，执行后清理。传输 Token 仅传给该子进程，不打印或写入项目配置。MCP 只传连接和任务信息，文件字节直接经 HTTP 传输。

助手固定使用所选 MCP 主机的文件服务，不允许通过转发参数覆盖 `--url` 或 Token 来源。需要连接另一台设备时，改助手的 `--url` / `WINDOWS_MCP_URL` 并使用对应 MCP Token。

若环境已有受信任的独立 CLI，也可直接运行 `uv run --script windows-mcp-transfer.py …`，通过 `WINDOWS_MCP_TRANSFER_URL` 与 `WINDOWS_MCP_TRANSFER_TOKEN` 或 `--token-stdin` 配置。不要为方便执行而把获取的 Token 保存到文档或仓库。

## 完成与进度

`--json` 产生 JSON Lines：`plan`、`progress`、`verified`、`completed` / `error`。检查：

- `progress.transferredBytes` 随传输增长，最后达到总量；零字节文件也应正常完成。
- 最终退出码为 0，最终结果 `ok=true`、`sha256Verified=true`。
- 通过 `FileTransfer.status` 或 CLI `status` 核对对应任务为 `completed`，无错误且已传字节等于总字节。仅有 100% 进度不能替代最终校验。

终端输出不可解析成单个 JSON 对象；逐行解析 `--json` 输出。普通 TTY 显示 Rich 进度条，重定向输出时退化为文本进度。主动取消或断开后，不自动重传未知状态的任务，先检查已发布文件和对应任务。

## 完整性、空目录和 ZIP

上传会做客户端 SHA-256 和服务端校验，校验失败不发布文件；下载先取得远程摘要，再流式落盘、比对大小和 SHA-256，成功后发布目标。同名文件不会被覆盖，目录传输逐文件发布，首版不提供自动续传或原子目录回滚。单次最多 10,000 个条目、1 TiB；拒绝链接、junction 和越界路径。

CLI 的目录下载逐条处理文件和目录，不依赖 ZIP，所以空文件、嵌套空目录和中文名称会被保留。HTTP ZIP 导出是另一路径：对已 URL 编码的共享相对目录请求 `GET /files/<目录>?zip`，使用**文件传输 Token**。每个路径分量单独编码，不能把中文、空格、`%` 或 `#` 原样拼到 URL。

0.2.7+ 的 ZIP 导出包含显式目录项；导出完全空的目录时包含该目录同名的 `目录名/` 条目。ZIP 使用有界流式压缩，最多 10,000 个条目、1 TiB 源数据，排除私有暂存目录及链接。测试 ZIP 时检查目录项以 `/` 结尾且确为目录，再解压到新的临时目录，比对文件摘要和目录清单。

HTTP 适用于可信局域网；SHA-256 校验完整性，不为 HTTP 内容提供加密。

## 直接使用 MCP 的服务端发送

只有用户要把**服务端本地文件**发送给另一台管理器时才使用：

```json
{"action":"add_peer","peer":{"name":"接收设备","url":"http://接收设备IP:8002","token":"从接收设备安全取得的传输Token"}}
{"action":"send","peer_id":"status返回的设备ID","paths":["D:\\build\\app"]}
{"action":"status"}
{"action":"cancel","task_id":"send返回的任务ID"}
```

配对信息含 Token，只在已有授权范围内保存，报告时隐藏。`send` 返回后台任务 ID，轮询状态确认字节进度和校验完成。它不读取客户端的同名 `D:\build\app`。

## 排错

- 找不到 `FileTransfer`：后端需由本项目管理器启动；独立官方后端不提供管理器的文件服务桥接。
- 401：区分 MCP Token 与传输 Token；重新从目标设备获取连接信息。
- 网络不可达：核对实际端口、局域网开关和防火墙现状；不要自动修改防火墙。
- 409 / 目标已存在：选择新的目标名称或目录，不自动删除原文件。
- SHA-256 不匹配：保留错误与任务 ID，检查来源文件是否变化，不声明成功、不重复发布损坏文件。
