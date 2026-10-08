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
  ffrm delete <路径>... [--unlock] [--force] [--yes] [--recursive]",
        version = env!("CARGO_PKG_VERSION")
    )
}

const HELP_TAIL: &str = "

命令:
  status   列出占用该文件的进程
  unlock   让占用进程退出以释放文件
  delete   删除文件或目录

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

pub fn execute(command: &Command) -> Result<(), String> {
    match command.action {
        Action::Status => status(command),
        Action::Unlock => unlock(command),
        Action::Delete => delete(command),
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

fn status(command: &Command) -> Result<(), String> {
    for path in &command.paths {
        print_inspection(&inspect(path)?)?;
    }
    Ok(())
}

fn unlock(command: &Command) -> Result<(), String> {
    let inspections = inspect_all(command)?;
    let lockers = all_lockers(&inspections);
    if inspections.iter().any(|item| item.list_denied) && lockers.is_empty() {
        if inspections.iter().any(|item| item.sharing_violation) {
            return Err("文件正被占用，但没有权限列出占用进程，无法解除占用。".to_string());
        }
        emit("没有权限列出占用进程，也没有检测到文件被占用。");
        return Ok(());
    }
    if lockers.is_empty() && inspections.iter().all(|item| !item.sharing_violation) {
        emit("没有进程占用这些文件。");
        return Ok(());
    }
    if let Some(reason) = critical_block(&lockers) {
        return Err(reason);
    }
    if lockers.is_empty() {
        return Err("文件正被占用，但没有列出可关闭的进程。请先关闭相关程序。".to_string());
    }
    emit("将关闭以下程序以释放文件:");
    print_lockers(&lockers);
    confirm(command.yes)?;
    for inspection in &inspections {
        if inspection.lockers.is_empty() {
            continue;
        }
        release(&inspection.path, command.force)?;
        let again = inspect(&inspection.path)?;
        if again.sharing_violation || !again.lockers.is_empty() {
            return Err(format!("{} 仍被占用", inspection.path.display()));
        }
        emit(&format!("已释放 {}", inspection.path.display()));
    }
    Ok(())
}

fn delete(command: &Command) -> Result<(), String> {
    for path in &command.paths {
        if let Some(reason) = block_reason(path) {
            return Err(format!("{}: {reason}", path.display()));
        }
    }
    let inspections = inspect_all(command)?;
    let lockers = all_lockers(&inspections);
    if inspections
        .iter()
        .any(|item| item.list_denied && item.sharing_violation && item.lockers.is_empty())
    {
        return Err("文件正被占用，但没有权限列出占用进程。".to_string());
    }
    let busy = inspections.iter().any(|item| item.sharing_violation) || !lockers.is_empty();
    if busy && !command.unlock {
        for inspection in &inspections {
            print_inspection(inspection)?;
        }
        return Err("文件被占用。确认要关闭占用程序并删除时，请加上 --unlock。".to_string());
    }
    if let Some(reason) = critical_block(&lockers) {
        return Err(reason);
    }
    if command.unlock && lockers.is_empty() && busy {
        return Err("文件正被占用，但没有列出可关闭的进程。请先关闭相关程序。".to_string());
    }
    emit("将删除:");
    for inspection in &inspections {
        emit(&format!("  {}", inspection.path.display()));
    }
    if command.unlock && !lockers.is_empty() {
        emit("将关闭以下程序:");
        print_lockers(&lockers);
    }
    confirm(command.yes)?;
    for inspection in &inspections {
        if command.unlock && !inspection.lockers.is_empty() {
            release(&inspection.path, command.force)?;
        }
        let removed = delete_path(&inspection.path, command.recursive)?;
        emit(&format!("已删除 {}", removed.display()));
    }
    Ok(())
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

fn print_inspection(inspection: &Inspection) -> Result<(), String> {
    emit(&inspection.path.display().to_string());
    if inspection.list_denied && inspection.lockers.is_empty() {
        emit("  没有权限列出占用进程");
    }
    if inspection.lockers.is_empty() && !inspection.sharing_violation {
        if !inspection.list_denied {
            emit("  没有进程占用此文件");
        }
        return Ok(());
    }
    if inspection.lockers.is_empty() {
        emit("  文件正被占用，但没有列出可关闭的进程");
        return Ok(());
    }
    print_lockers(&inspection.lockers);
    Ok(())
}

fn print_lockers(lockers: &[Locker]) {
    for locker in lockers {
        let mut line = format!("  pid {}  {}", locker.pid, display_name(locker));
        if let Some(image) = &locker.image {
            line.push_str("  ");
            line.push_str(image);
        }
        if is_critical(locker) {
            line.push_str("  [关键系统进程]");
        }
        emit(&line);
    }
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
