mod config;
mod gui;
mod menu;
mod update;

use std::process::ExitCode;

use windows_sys::Win32::System::Console::FreeConsole;

fn launch_window(args: &[std::ffi::OsString]) -> bool {
    match args.first() {
        None => true,
        Some(arg) => {
            let text = arg.to_string_lossy();
            !text.starts_with('-') && !matches!(text.as_ref(), "status" | "unlock" | "delete")
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if launch_window(&args) {
        let paths = args.into_iter().map(std::path::PathBuf::from).collect();
        unsafe { FreeConsole() };
        gui::run(paths);
        return ExitCode::SUCCESS;
    }
    let parsed = match ffrm::parse_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            ffrm::emit_err(&error);
            ffrm::emit_err("运行 ffrm --help 查看用法");
            return ExitCode::from(1);
        }
    };
    match parsed {
        ffrm::Parsed::Help => {
            print!("{}", ffrm::help_text());
            ExitCode::SUCCESS
        }
        ffrm::Parsed::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ffrm::Parsed::Run(command) => match ffrm::execute(&command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                ffrm::emit_err(&error);
                ExitCode::from(1)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::launch_window;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().copied().map(OsString::from).collect()
    }

    #[test]
    fn paths_open_the_window() {
        assert!(launch_window(&[]));
        assert!(launch_window(&args(&[r"D:\temp\a.txt"])));
        assert!(!launch_window(&args(&["status", r"D:\temp\a.txt"])));
        assert!(!launch_window(&args(&["--help"])));
        assert!(!launch_window(&args(&["--version"])));
    }
}
