use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};

use ffrm::{Action, Command};

pub fn serve() -> ExitCode {
    if io::stdin().is_terminal() {
        ffrm::emit_err("ffrm mcp 正在等待编辑器从标准输入发送消息。");
    }
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    loop {
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => return ExitCode::SUCCESS,
            Err(_) => return ExitCode::from(1),
        };
        if let Some(response) = reply(&message.body) {
            if write_message(message.content_length, &response).is_err() {
                return ExitCode::from(1);
            }
        }
    }
}

pub fn reply(message: &str) -> Option<String> {
    let value: Value = match serde_json::from_str(message) {
        Ok(value) => value,
        Err(_) => {
            return Some(respond(Value::Null, Err((-32700, "无法解析 JSON".to_string()))));
        }
    };
    let Some(id) = value.get("id").filter(|id| !id.is_null()).cloned() else {
        return None;
    };
    let method = value.get("method").and_then(|item| item.as_str()).unwrap_or("");
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    Some(respond(id, dispatch(method, &params)))
}

fn dispatch(method: &str, params: &Value) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools()),
        "tools/call" => Ok(call_tool(params)),
        "logging/setLevel" => Ok(json!({})),
        _ => Err((-32601, format!("未知方法: {method}"))),
    }
}

fn initialize(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(|item| item.as_str())
        .filter(|version| !version.is_empty())
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "ffrm",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "查看谁占用了本地文件，解除占用，然后删除。unlock 和 delete 必须把 confirm 设为 true。文件被占用时，delete 必须同时把 unlock 设为 true。非空目录必须把 recursive 设为 true。force 必须和 unlock 一起用。关键系统进程会被拒绝。"
    })
}

fn tools() -> Value {
    json!({
        "tools": [
            {
                "name": "status",
                "description": "列出占用该路径的进程。目录仍被占用但没有列出进程时，会再查找当前目录正好是该文件夹的进程。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件或文件夹的完整路径" }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            },
            {
                "name": "unlock",
                "description": "让占用进程退出以释放文件。进程没有退出时会结束该进程。关键系统进程会被拒绝。confirm 必须为 true 才会执行。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件或文件夹的完整路径" },
                        "confirm": { "type": "boolean", "description": "必须为 true 才会执行" },
                        "force": { "type": "boolean", "description": "请求退出后直接强制结束" }
                    },
                    "required": ["path", "confirm"],
                    "additionalProperties": false
                }
            },
            {
                "name": "delete",
                "description": "删除文件或目录。confirm 必须为 true 才会执行。路径被占用时必须同时把 unlock 设为 true。目录里还有内容时必须把 recursive 设为 true。force 必须和 unlock 一起用。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件或文件夹的完整路径" },
                        "confirm": { "type": "boolean", "description": "必须为 true 才会执行" },
                        "unlock": { "type": "boolean", "description": "删除前先解除占用" },
                        "force": { "type": "boolean", "description": "解除占用时，请求退出后直接强制结束" },
                        "recursive": { "type": "boolean", "description": "删除非空目录" }
                    },
                    "required": ["path", "confirm"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

fn call_tool(params: &Value) -> Value {
    let name = params.get("name").and_then(|item| item.as_str()).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return tool_error("arguments 必须是对象。".to_string());
    }
    match name {
        "status" => status_tool(&arguments),
        "unlock" => unlock_tool(&arguments),
        "delete" => delete_tool(&arguments),
        _ => tool_error(format!("未知工具: {name}。可用的工具是 status、unlock、delete。")),
    }
}

fn status_tool(arguments: &Value) -> Value {
    let path = match path_of(arguments) {
        Ok(path) => path,
        Err(error) => return tool_error(error),
    };
    run_tool(Command {
        action: Action::Status,
        paths: vec![path],
        unlock: false,
        force: false,
        yes: true,
        recursive: false,
    })
}

fn unlock_tool(arguments: &Value) -> Value {
    if !flag(arguments, "confirm") {
        return tool_error("没有执行。把 confirm 设为 true 才会解除占用。".to_string());
    }
    let path = match path_of(arguments) {
        Ok(path) => path,
        Err(error) => return tool_error(error),
    };
    run_tool(Command {
        action: Action::Unlock,
        paths: vec![path],
        unlock: false,
        force: flag(arguments, "force"),
        yes: true,
        recursive: false,
    })
}

fn delete_tool(arguments: &Value) -> Value {
    if !flag(arguments, "confirm") {
        return tool_error("没有执行。把 confirm 设为 true 才会删除。".to_string());
    }
    let unlock = flag(arguments, "unlock");
    let force = flag(arguments, "force");
    if force && !unlock {
        return tool_error("delete 使用 force 时必须同时把 unlock 设为 true。".to_string());
    }
    let path = match path_of(arguments) {
        Ok(path) => path,
        Err(error) => return tool_error(error),
    };
    run_tool(Command {
        action: Action::Delete,
        paths: vec![path],
        unlock,
        force,
        yes: true,
        recursive: flag(arguments, "recursive"),
    })
}

fn run_tool(command: Command) -> Value {
    let report = ffrm::run(&command);
    let is_error = report.error.is_some();
    let text = match report.error {
        Some(error) if report.output.is_empty() => error,
        Some(error) => format!("{}\n{error}", report.output),
        None => report.output,
    };
    tool_text(text, is_error)
}

fn path_of(arguments: &Value) -> Result<PathBuf, String> {
    match arguments.get("path").and_then(|item| item.as_str()) {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err("缺少 path。".to_string()),
    }
}

fn flag(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(|item| item.as_bool())
        .unwrap_or(false)
}

fn tool_text(text: String, is_error: bool) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": text }]
    });
    if is_error {
        result["isError"] = json!(true);
    }
    result
}

fn tool_error(text: String) -> Value {
    tool_text(text, true)
}

fn respond(id: Value, result: Result<Value, (i32, String)>) -> String {
    let message = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    };
    message.to_string()
}

struct Incoming {
    body: String,
    content_length: bool,
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Incoming>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') {
            return Ok(Some(Incoming {
                body: trimmed.to_string(),
                content_length: false,
            }));
        }
        let Some(len_text) = trimmed.strip_prefix("Content-Length:") else {
            continue;
        };
        let len: usize = len_text
            .trim()
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "缺少消息正文",
                ));
            }
            if line.trim().is_empty() {
                break;
            }
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        let body = String::from_utf8(buf)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        return Ok(Some(Incoming {
            body,
            content_length: true,
        }));
    }
}

fn write_message(content_length: bool, body: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if content_length {
        write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
        out.write_all(body.as_bytes())?;
    } else {
        out.write_all(body.as_bytes())?;
        out.write_all(b"\n")?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let response = reply(&request.to_string()).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn tool_message(response: &Value) -> &str {
        response["result"]["content"][0]["text"].as_str().unwrap()
    }

    #[test]
    fn initialize_names_the_server() {
        let response = reply(
            r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"0"}}}"#,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["id"].as_i64(), Some(7));
        assert_eq!(parsed["result"]["serverInfo"]["name"].as_str(), Some("ffrm"));
        assert_eq!(parsed["result"]["protocolVersion"].as_str(), Some("2024-11-05"));
    }

    #[test]
    fn initialized_notification_has_no_reply() {
        assert!(reply(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn tools_are_status_unlock_and_delete() {
        let response = reply(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        let names: Vec<_> = parsed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["status", "unlock", "delete"]);
    }

    #[test]
    fn status_reports_a_free_file() {
        let file = std::env::temp_dir().join(format!("ffrm-mcp-status-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let response = call("status", json!({ "path": file.display().to_string() }));
        assert!(response.get("error").is_none());
        assert!(tool_message(&response).contains("没有进程占用此文件"));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn delete_without_confirm_keeps_the_file() {
        let file = std::env::temp_dir().join(format!("ffrm-mcp-keep-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let response = call(
            "delete",
            json!({ "path": file.display().to_string(), "confirm": false }),
        );
        assert_eq!(response["result"]["isError"].as_bool(), Some(true));
        assert!(tool_message(&response).contains("confirm"));
        assert!(file.exists());
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn delete_with_confirm_removes_a_free_file() {
        let file = std::env::temp_dir().join(format!("ffrm-mcp-drop-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let response = call(
            "delete",
            json!({ "path": file.display().to_string(), "confirm": true }),
        );
        assert!(response.get("result").unwrap().get("isError").is_none());
        assert!(tool_message(&response).contains("已删除"));
        assert!(!file.exists());
    }

    #[test]
    fn force_delete_without_unlock_does_nothing() {
        let file = std::env::temp_dir().join(format!("ffrm-mcp-force-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let response = call(
            "delete",
            json!({ "path": file.display().to_string(), "confirm": true, "force": true }),
        );
        assert_eq!(response["result"]["isError"].as_bool(), Some(true));
        assert!(tool_message(&response).contains("unlock"));
        assert!(file.exists());
        std::fs::remove_file(&file).unwrap();
    }
}
