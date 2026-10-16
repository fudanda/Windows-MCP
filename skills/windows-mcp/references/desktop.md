# 桌面操作

本参考针对本项目 Windows-MCP Manager 0.2.9 的已验证行为。其他版本先查看工具 schema；独立官方后端可能没有 `FileTransfer`，也可能未启用 `ControlStatus`。

## 直接调用或使用助手

下列 JSON 是工具参数，适用于已配置的 MCP 工具。没有原生工具入口时，用：

```powershell
$client = '<skill-dir>/scripts/windows_mcp_client.py'
uv run --no-project --script $client tools
uv run --no-project --script $client call DisplayInventory
uv run --no-project --script $client --output-dir './artifacts/remote' call Screenshot --args-json '{"use_annotation":false}'
```

脚本从 `WINDOWS_MCP_URL` 读取地址，从 `WINDOWS_MCP_TOKEN`、`--token-stdin` 或交互式隐藏输入读取原始 Token。`--url` 可指定不同设备。它默认将图片保存到当前项目 `artifacts/windows-mcp/`，输出绝对路径和 JSON，不将图片 Base64 打印到终端；同一次执行遇到工具错误返回非零退出码。

本机助手的 MCP SSE 消息上限默认为 1 GiB，可用 `--max-sse-event-size` 覆盖。这个值只作用于助手，不会修改其他 MCP 客户端或服务器设置。

## 常用参数

| 工具 | 参数示例 | 核对方式 |
| --- | --- | --- |
| `ControlStatus` | `{}` | `ready` / `ai` 可继续；`user` 等待；不可用时报告状态 |
| `DisplayInventory` | `{}` | 屏幕索引、虚拟桌面边界、分辨率、DPI 与缩放 |
| `Screenshot` | `{"use_annotation":false}` | 实际图片与返回的尺寸 |
| `Screenshot` | `{"region":[100,200,900,700],"use_annotation":false}` | 该区域宽 800、高 500 |
| `Snapshot` | `{"use_vision":true,"use_ui_tree":true,"use_annotation":false}` | 窗口、UI 元素及坐标；需要时才请求 UI 树 |
| `App` | `{"mode":"switch","name":"目标窗口的完整标题"}` | 重新截图或 Snapshot，确认实际目标 |
| `Click` | `{"loc":[400,300],"button":"left","clicks":1}` | 焦点、按钮事件或状态发生预期变化 |
| `Type` | `{"loc":[400,300],"text":"中文测试 English 123","clear":true,"press_enter":false}` | 读取实际字段或核对可见文本 |
| `Shortcut` | `{"shortcut":"ctrl+a"}` | 选择范围变化；退格等操作只作用于已确认的目标 |

参数中的坐标仅为格式示例，执行前从当前画面或元素定位获取。`Screenshot` 的 `display=[0]` 选择 Windows 活跃显示器索引；`region` 与 `display` 同时存在时，以 `region` 为准。

图片可能被缩小到 1920×1080 以内。把图中坐标换回虚拟桌面坐标时：`screen_x = region_left + image_x / scale`，`screen_y = region_top + image_y / scale`。以返回元数据中的原尺寸、比例、区域为准；全桌面还要考虑负坐标的显示器原点。

窗口切换使用实时列表，先忽略大小写精确匹配，再接受唯一的子串匹配。不存在或有多个候选时返回 MCP 错误，不做模糊猜测。隐藏窗口可能不在可切换列表中；不要把隐藏目标的拒绝当成切换到了别的应用。需要恢复应用时使用它正常的启动/显示入口，并再次定位。

## 受控测试

用户要求测试输入时，使用专用的临时输入窗口、未保存的测试文档或项目提供的 fixture。先观察并选中输入区域，再输入。验证按钮点击、英文、中文、数字和标点时，读取实际结果而非只看工具的成功提示。测试完成后关闭自己的窗口，并处理保存提示以免生成无关文件。

`Type` 的中文使用粘贴路径；测试拼音输入法属于另一项任务。不要以 `Type` 成功声称逐键输入、输入法组合或所有应用兼容均已验证。

## 常见问题

- **401**：核对设备、最新 MCP Token 和 `Authorization: Bearer …`；不要把凭据放在 URL 中。
- **截图没有元素或 active window**：快速截图省略 UI 枚举，按需使用 `Snapshot`；截图非空仍可能是有效结果。
- **`cv2` 缺失**：0.2.9 的截图链会自动回退到 Pillow。旧版本出现该错误时优先升级管理器，不要求客户端安装 OpenCV。
- **区域不正确**：确认右/下边界而非宽/高；结合 `DisplayInventory` 核对多屏原点与缩放。
- **窗口未找到 / Ambiguous**：重新观察完整标题，不自动放宽为模糊匹配。
- **用户接管**：停止输入，等空闲后重查；文件传输不需要桌面输入控制权，可独立进行。
