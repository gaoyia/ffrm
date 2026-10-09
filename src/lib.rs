mod lock;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use lock::critical_block;

pub use lock::{
    block_reason, delete_path, display_name, inspect, is_critical, release, Inspection, Locker,
};

fn help_body() -> String {
    format!(
        "\
ffrm {version}
查看谁占用了本地文件，解除占用，并删除文件。
不带参数，或直接传入路径时，打开桌面窗口。

用法:
  ffrm
  ffrm <路径>...
  ffrm status <路径>...
  ffrm unlock <路径>... [--force] [--yes]
  ffrm delete <路径>... [--unlock] [--force] [--yes] [--recursive]
  ffrm mcp",
        version = env!("CARGO_PKG_VERSION")
    )
}

const HELP_TAIL: &str = "

命令:
  status   列出占用该文件的进程
  unlock   让占用进程退出以释放文件
  delete   删除文件或目录
  mcp      启动 MCP 服务，供编辑器调用上面三个命令

选项:
  --unlock       delete 时若文件被占用，先解除占用
  --force, -f    进程无响应时强制结束
  --yes, -y      不再询问确认
  --recursive, -r
                 删除非空目录
  -h, --help     显示帮助
  -V, --version  显示版本

直接传入路径只会打开窗口并加入队列，不会删除。
不会删除磁盘根目录、Windows 目录、用户主目录或 Program Files 本身。
unlock 会关闭正在使用该文件的程序。程序若没有退出，会结束该进程。关键系统进程会被拒绝。
目录仍被占用但没有列出进程时，会再查找当前目录正好是该文件夹的进程。
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub action: Action,
    pub paths: Vec<PathBuf>,
    pub unlock: bool,
    pub force: bool,
    pub yes: bool,
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Unlock,
    Delete,
}

#[derive(Debug)]
pub enum Parsed {
    Help,
    Version,
    Run(Command),
}

pub fn parse_args<I, S>(args: I) -> Result<Parsed, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut action = None;
    let mut paths = Vec::new();
    let mut unlock = false;
    let mut force = false;
    let mut yes = false;
    let mut recursive = false;
    let mut positional = false;

    for arg in args {
        let arg = arg.into();
        let text = arg.to_string_lossy();
        if !positional && text == "--" {
            positional = true;
            continue;
        }
        if !positional && (text == "-h" || text == "--help") {
            return Ok(Parsed::Help);
        }
        if !positional && (text == "-V" || text == "--version") {
            return Ok(Parsed::Version);
        }
        if !positional && text.starts_with('-') {
            match text.as_ref() {
                "--unlock" => unlock = true,
                "--force" | "-f" => force = true,
                "--yes" | "-y" => yes = true,
                "--recursive" | "-r" => recursive = true,
                other => return Err(format!("未知选项: {other}")),
            }
            continue;
        }
        if action.is_none() && !positional {
            action = Some(parse_action(&text)?);
            continue;
        }
        paths.push(PathBuf::from(arg));
    }

    let action = action.ok_or_else(|| "请指定 status、unlock 或 delete".to_string())?;
    if paths.is_empty() {
        return Err("请至少提供一个路径".to_string());
    }
    match action {
        Action::Status if unlock || force || recursive => {
            return Err("status 只接受路径".to_string());
        }
        Action::Unlock if unlock || recursive => {
            return Err("unlock 不接受 --unlock 或 --recursive".to_string());
        }
        Action::Delete if force && !unlock => {
            return Err("delete 使用 --force 时必须同时加上 --unlock".to_string());
        }
        _ => {}
    }

    Ok(Parsed::Run(Command {
        action,
        paths,
        unlock,
        force,
        yes,
        recursive,
    }))
}

pub struct Report {
    pub output: String,
    pub error: Option<String>,
}

pub fn execute(command: &Command) -> Result<(), String> {
    let report = run(command);
    if !report.output.is_empty() {
        for line in report.output.split('\n') {
            emit(line);
        }
    }
    match report.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub fn run(command: &Command) -> Report {
    match command.action {
        Action::Status => status(command),
        Action::Unlock => unlock(command),
        Action::Delete => delete(command),
    }
}

fn lines_ok(lines: Vec<String>) -> Report {
    Report {
        output: lines.join("\n"),
        error: None,
    }
}

fn lines_err(lines: Vec<String>, error: String) -> Report {
    Report {
        output: lines.join("\n"),
        error: Some(error),
    }
}

pub fn help_text() -> String {
    format!("{}{HELP_TAIL}", help_body())
}

fn parse_action(text: &str) -> Result<Action, String> {
    match text {
        "status" => Ok(Action::Status),
        "unlock" => Ok(Action::Unlock),
        "delete" => Ok(Action::Delete),
        other => Err(format!("未知命令: {other}")),
    }
}

fn status(command: &Command) -> Report {
    let mut lines = Vec::new();
    for path in &command.paths {
        match inspect(path) {
            Ok(inspection) => lines.extend(inspection_lines(&inspection)),
            Err(error) => return lines_err(lines, error),
        }
    }
    lines_ok(lines)
}

fn unlock(command: &Command) -> Report {
    let mut lines = Vec::new();
    let inspections = match inspect_all(command) {
        Ok(inspections) => inspections,
        Err(error) => return lines_err(lines, error),
    };
    let lockers = all_lockers(&inspections);
    if inspections.iter().any(|item| item.list_denied) && lockers.is_empty() {
        if inspections.iter().any(|item| item.sharing_violation) {
            return lines_err(
                lines,
                "文件正被占用，但没有权限列出占用进程，无法解除占用。".to_string(),
            );
        }
        lines.push("没有权限列出占用进程，也没有检测到文件被占用。".to_string());
        return lines_ok(lines);
    }
    if lockers.is_empty() && inspections.iter().all(|item| !item.sharing_violation) {
        lines.push("没有进程占用这些文件。".to_string());
        return lines_ok(lines);
    }
    if let Some(reason) = critical_block(&lockers) {
        return lines_err(lines, reason);
    }
    if lockers.is_empty() {
        return lines_err(
            lines,
            "文件正被占用，但没有列出可关闭的进程。请先关闭相关程序。".to_string(),
        );
    }
    lines.push("将关闭以下程序以释放文件:".to_string());
    lines.extend(locker_lines(&lockers));
    if let Err(error) = confirm(command.yes) {
        return lines_err(lines, error);
    }
    for inspection in &inspections {
        if inspection.lockers.is_empty() {
            continue;
        }
        if let Err(error) = release(&inspection.path, command.force) {
            return lines_err(lines, error);
        }
        let again = match inspect(&inspection.path) {
            Ok(again) => again,
            Err(error) => return lines_err(lines, error),
        };
        if again.sharing_violation || !again.lockers.is_empty() {
            return lines_err(lines, format!("{} 仍被占用", inspection.path.display()));
        }
        lines.push(format!("已释放 {}", inspection.path.display()));
    }
    lines_ok(lines)
}

fn delete(command: &Command) -> Report {
    let mut lines = Vec::new();
    for path in &command.paths {
        if let Some(reason) = block_reason(path) {
            return lines_err(lines, format!("{}: {reason}", path.display()));
        }
    }
    let inspections = match inspect_all(command) {
        Ok(inspections) => inspections,
        Err(error) => return lines_err(lines, error),
    };
    let lockers = all_lockers(&inspections);
    if inspections
        .iter()
        .any(|item| item.list_denied && item.sharing_violation && item.lockers.is_empty())
    {
        return lines_err(lines, "文件正被占用，但没有权限列出占用进程。".to_string());
    }
    let busy = inspections.iter().any(|item| item.sharing_violation) || !lockers.is_empty();
    if busy && !command.unlock {
        for inspection in &inspections {
            lines.extend(inspection_lines(inspection));
        }
        return lines_err(
            lines,
            "文件被占用。确认要关闭占用程序并删除时，请加上 --unlock。".to_string(),
        );
    }
    if let Some(reason) = critical_block(&lockers) {
        return lines_err(lines, reason);
    }
    if command.unlock && lockers.is_empty() && busy {
        return lines_err(
            lines,
            "文件正被占用，但没有列出可关闭的进程。请先关闭相关程序。".to_string(),
        );
    }
    lines.push("将删除:".to_string());
    for inspection in &inspections {
        lines.push(format!("  {}", inspection.path.display()));
    }
    if command.unlock && !lockers.is_empty() {
        lines.push("将关闭以下程序:".to_string());
        lines.extend(locker_lines(&lockers));
    }
    if let Err(error) = confirm(command.yes) {
        return lines_err(lines, error);
    }
    for inspection in &inspections {
        if command.unlock && !inspection.lockers.is_empty() {
            if let Err(error) = release(&inspection.path, command.force) {
                return lines_err(lines, error);
            }
        }
        match delete_path(&inspection.path, command.recursive) {
            Ok(removed) => lines.push(format!("已删除 {}", removed.display())),
            Err(error) => return lines_err(lines, error),
        }
    }
    lines_ok(lines)
}

fn inspect_all(command: &Command) -> Result<Vec<Inspection>, String> {
    command.paths.iter().map(|path| inspect(path)).collect()
}

fn all_lockers(inspections: &[Inspection]) -> Vec<Locker> {
    let mut lockers = Vec::new();
    for inspection in inspections {
        for locker in &inspection.lockers {
            if !lockers
                .iter()
                .any(|existing: &Locker| existing.pid == locker.pid)
            {
                lockers.push(locker.clone());
            }
        }
    }
    lockers
}

fn inspection_lines(inspection: &Inspection) -> Vec<String> {
    let mut lines = vec![inspection.path.display().to_string()];
    if inspection.list_denied && inspection.lockers.is_empty() {
        lines.push("  没有权限列出占用进程".to_string());
    }
    if inspection.lockers.is_empty() && !inspection.sharing_violation {
        if !inspection.list_denied {
            lines.push("  没有进程占用此文件".to_string());
        }
        return lines;
    }
    if inspection.lockers.is_empty() {
        lines.push("  文件正被占用，但没有列出可关闭的进程".to_string());
        return lines;
    }
    lines.extend(locker_lines(&inspection.lockers));
    lines
}

fn locker_lines(lockers: &[Locker]) -> Vec<String> {
    lockers
        .iter()
        .map(|locker| {
            let mut line = format!("  pid {}  {}", locker.pid, display_name(locker));
            if let Some(image) = &locker.image {
                line.push_str("  ");
                line.push_str(image);
            }
            if is_critical(locker) {
                line.push_str("  [关键系统进程]");
            }
            line
        })
        .collect()
}

fn confirm(yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err("当前不是交互终端，请加上 --yes".to_string());
    }
    emit_raw_err("输入 y 继续: ")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|err| format!("无法读取确认: {err}"))?;
    let answer = line.trim();
    if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") || answer == "是" {
        Ok(())
    } else {
        Err("已取消".to_string())
    }
}

fn emit(line: &str) {
    let _ = write_line(io::stdout(), line);
}

fn emit_raw_err(text: &str) -> Result<(), String> {
    if write_console(&io::stderr(), text) {
        return Ok(());
    }
    let mut error = io::stderr();
    error
        .write_all(text.as_bytes())
        .and_then(|()| error.flush())
        .map_err(|err| format!("无法写出提示: {err}"))
}

pub fn emit_err(line: &str) {
    let _ = write_line(io::stderr(), line);
}

fn write_line<T>(target: T, line: &str) -> io::Result<()>
where
    T: Write + std::os::windows::io::AsRawHandle,
{
    let mut text = String::from(line);
    text.push('\n');
    if write_console(&target, &text) {
        return Ok(());
    }
    let mut target = target;
    target.write_all(text.as_bytes())?;
    target.flush()
}

fn write_console(target: &impl std::os::windows::io::AsRawHandle, text: &str) -> bool {
    use windows_sys::Win32::System::Console::WriteConsoleW;

    let handle = target.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    if handle.is_null() {
        return false;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut written = 0u32;
    let ok = unsafe {
        WriteConsoleW(
            handle,
            wide.as_ptr(),
            wide.len() as u32,
            &mut written,
            std::ptr::null(),
        )
    };
    ok != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_delete_command() {
        let parsed = parse_args([
            "delete",
            r"D:\temp\a.txt",
            "--unlock",
            "--force",
            "--yes",
            "--recursive",
        ])
        .unwrap();
        match parsed {
            Parsed::Run(command) => {
                assert_eq!(command.action, Action::Delete);
                assert!(command.unlock && command.force && command.yes && command.recursive);
                assert_eq!(command.paths, vec![PathBuf::from(r"D:\temp\a.txt")]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_force_delete_without_unlock() {
        let error = parse_args(["delete", r"D:\temp\a.txt", "--force"]).unwrap_err();
        assert!(error.contains("--unlock"));
    }

    #[test]
    fn help_and_missing_path() {
        assert!(matches!(parse_args(["--help"]), Ok(Parsed::Help)));
        assert!(parse_args(["status"]).is_err());
    }

    #[test]
    fn deletes_a_directory_held_only_as_a_current_directory() {
        let dir = std::env::temp_dir().join(format!("ffrm-cwd-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 60 127.0.0.1 > nul"])
            .current_dir(&dir)
            .spawn()
            .unwrap();
        let result = execute(&Command {
            action: Action::Delete,
            paths: vec![dir.clone()],
            unlock: true,
            force: true,
            yes: true,
            recursive: false,
        });
        let _ = child.kill();
        let _ = child.wait();
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result.unwrap();
        assert!(!dir.exists());
    }
}
