# ffrm-mcp

Windows 上的 [ffrm](https://github.com/gaoyia/ffrm) MCP 服务。安装时下载对应版本的 `ffrm.exe`。

工具：

- `status`：列出占用该路径的进程
- `unlock`：解除占用。`confirm` 必须为 `true`
- `delete`：删除文件或目录。`confirm` 必须为 `true`

`unlock` 会关闭占用程序。关键系统进程会被拒绝。

```json
{
  "mcpServers": {
    "ffrm": {
      "command": "npx",
      "args": ["-y", "ffrm-mcp"]
    }
  }
}
```

已有 `ffrm.exe` 时，可以设置 `FFRM_BIN` 指向它，安装仍会下载发布包里的那一份。
