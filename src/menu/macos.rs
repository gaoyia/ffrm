//! 访达不能把普通程序放进右键菜单第一层。这里安装的是当前用户的「服务」。

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Lang;

const BUNDLE_ID: &str = "com.gaoyia.ffrm.finder-service";
const WORKFLOW_NAME: &str = "ffrm.workflow";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuState {
    Absent,
    Current,
    Other,
}

pub fn state() -> MenuState {
    let Ok(exe) = std::env::current_exe() else {
        return MenuState::Absent;
    };
    classify(&service_dir(), &exe)
}

pub fn install(label: &str, lang: Lang) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|_| missing_exe(lang))?;
    let dir = service_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|err| io_fail(lang, &err))?;
    }
    write_service(&dir, label, &exe).map_err(|err| io_fail(lang, &err))?;
    sign_service(&dir);
    if let Err(error) = refresh() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }
    enable_service(label);
    Ok(())
}

pub fn remove(lang: Lang) -> Result<(), String> {
    let dir = service_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|err| io_fail(lang, &err))?;
    }
    refresh()?;
    Ok(())
}

pub fn extra_hint(lang: Lang) -> Option<&'static str> {
    Some(match lang {
        Lang::Zh => "会出现在访达右键菜单的「服务」里。系统设置中的位置是「键盘 → 键盘快捷键 → 服务 → 文件和文件夹」。",
        Lang::En => "It appears under Services in the Finder context menu. In System Settings, look under Keyboard → Keyboard Shortcuts → Services → Files and Folders.",
    })
}

fn services_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library").join("Services"))
}

fn service_dir() -> PathBuf {
    services_root()
        .unwrap_or_else(|| PathBuf::from("/var/empty"))
        .join(WORKFLOW_NAME)
}

fn classify(dir: &Path, exe: &Path) -> MenuState {
    if !dir.exists() {
        return MenuState::Absent;
    }
    let Ok(stored) = std::fs::read(dir.join("Contents/Resources/ffrm-command")) else {
        return MenuState::Other;
    };
    let stored = PathBuf::from(std::ffi::OsString::from_vec(stored));
    if same_exe(&stored, exe) {
        MenuState::Current
    } else {
        MenuState::Other
    }
}

fn write_service(dir: &Path, label: &str, exe: &Path) -> std::io::Result<()> {
    let resources = dir.join("Contents/Resources");
    std::fs::create_dir_all(&resources)?;
    std::fs::write(dir.join("Contents/Info.plist"), info_plist(label))?;
    std::fs::write(resources.join("document.wflow"), document_wflow(exe))?;
    std::fs::write(resources.join("ffrm-command"), exe.as_os_str().as_bytes())?;
    Ok(())
}

fn info_plist(label: &str) -> String {
    let label = xml_escape(label);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh_CN</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleName</key>
	<string>{label}</string>
	<key>CFBundleShortVersionString</key>
	<string>1.0</string>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{label}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.item</string>
				<string>public.folder</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

fn document_wflow(exe: &Path) -> String {
    let command = xml_escape(&command_script(exe));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{command}</string>
					<key>CheckedForUserDefaultShell</key>
					<false/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<false/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>B1C4E5A0-7E2A-4C1D-9A11-000000000001</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
					<string>Command</string>
					<string>Run</string>
				</array>
				<key>OutputUUID</key>
				<string>B1C4E5A0-7E2A-4C1D-9A11-000000000002</string>
				<key>UUID</key>
				<string>B1C4E5A0-7E2A-4C1D-9A11-000000000003</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict/>
				<key>isViewVisible</key>
				<integer>0</integer>
				<key>location</key>
				<string>309.000000:253.000000</string>
				<key>nibPath</key>
				<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>serviceApplicationBundleID</key>
		<string></string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#
    )
}

fn command_script(exe: &Path) -> String {
    if let Some(app) = app_bundle(exe) {
        // 直接执行包内程序会挂在终端上：程序坞显示终端图标，关窗口后进程也不退出。
        format!(
            "exec /usr/bin/open -n {} --args \"$@\"\n",
            shell_single_quote(&app.display().to_string())
        )
    } else {
        format!(
            "exec {} \"$@\"\n",
            shell_single_quote(&exe.display().to_string())
        )
    }
}

fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
}

fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn same_exe(stored: &Path, exe: &Path) -> bool {
    match (stored.canonicalize(), exe.canonicalize()) {
        (Ok(stored), Ok(exe)) => stored == exe,
        _ => stored == exe,
    }
}

fn sign_service(dir: &Path) {
    let Some(path) = dir.to_str() else {
        return;
    };
    // 下载或复制带来的隔离标记会让访达忽略这个服务。
    let _ = Command::new("/usr/bin/xattr").args(["-cr", path]).status();
    let _ = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", path])
        .status();
}

fn refresh() -> Result<(), String> {
    let program = Path::new("/System/Library/CoreServices/pbs");
    let update = Command::new(program).arg("-update").status();
    let flush = Command::new(program).arg("-flush").status();
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .args(["-f", "-R"])
        .arg(service_dir())
        .status();
    // 访达会缓存服务菜单，重开后才能看到新项目。
    let _ = Command::new("/usr/bin/killall").arg("Finder").status();
    if update.map(|status| status.success()).unwrap_or(false)
        || flush.map(|status| status.success()).unwrap_or(false)
    {
        return Ok(());
    }
    Err("无法刷新访达服务".to_string())
}

fn enable_service(label: &str) {
    let key = format!("{BUNDLE_ID} - {label} - runWorkflowAsService");
    let value = "{ \"enabled_context_menu\" = 1; \"enabled_services_menu\" = 1; }";
    let _ = Command::new("/usr/bin/defaults")
        .args(["write", "pbs", "NSServicesStatus", "-dict-add", &key, value])
        .status();
}

fn missing_exe(lang: Lang) -> String {
    match lang {
        Lang::Zh => "无法定位程序自身的路径".to_string(),
        Lang::En => "Could not find this program".to_string(),
    }
}

fn io_fail(lang: Lang, err: &std::io::Error) -> String {
    match lang {
        Lang::Zh => format!("无法修改访达服务：{err}"),
        Lang::En => format!("Could not change the Finder service: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_service_opens_the_bundle() {
        let script = command_script(Path::new("/Applications/ffrm.app/Contents/MacOS/ffrm"));
        assert_eq!(
            script,
            "exec /usr/bin/open -n '/Applications/ffrm.app' --args \"$@\"\n"
        );
    }

    #[test]
    fn portable_service_execs_the_binary() {
        let script = command_script(Path::new("/Users/me/Desktop/ffrm"));
        assert_eq!(script, "exec '/Users/me/Desktop/ffrm' \"$@\"\n");
    }

    #[test]
    fn quotes_paths_and_escapes_plist_text() {
        assert_eq!(shell_single_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_single_quote("/tmp/a'b"), "'/tmp/a'\\''b'");
        assert_eq!(xml_escape("A&B<C>"), "A&amp;B&lt;C&gt;");
    }

    #[test]
    fn bundle_records_this_program_and_pbs_can_read_it() {
        let root = std::env::temp_dir().join(format!("ffrm-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let workflow = root.join(WORKFLOW_NAME);
        let exe = std::env::current_exe().unwrap();
        write_service(&workflow, "强制解锁删除", &exe).unwrap();
        assert_eq!(classify(&workflow, &exe), MenuState::Current);
        assert_eq!(
            classify(&workflow, Path::new("/tmp/other-ffrm")),
            MenuState::Other
        );

        let output = Command::new("/System/Library/CoreServices/pbs")
            .args(["-read_bundle", workflow.to_str().unwrap()])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let plist = std::fs::read_to_string(workflow.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("强制解锁删除"), "{plist}");
        assert!(text.contains(BUNDLE_ID), "{text}");
        assert!(text.contains("public.folder"), "{text}");

        let note = root.join("got.txt");
        let script = root.join("record.sh");
        let sample = root.join("sample.txt");
        std::fs::write(&sample, b"sample").unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/bash\nprintf '%s\\n' \"$@\" > {}\n",
                shell_single_quote(&note.display().to_string())
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        write_service(&workflow, "强制解锁删除", &script).unwrap();
        let ran = Command::new("/usr/bin/automator")
            .arg("-i")
            .arg(&sample)
            .arg(&workflow)
            .output()
            .unwrap();
        let recorded = std::fs::read_to_string(&note).unwrap_or_default();
        assert!(
            recorded.contains(sample.to_str().unwrap()),
            "automator 没有把路径传给服务\n{recorded}\n{}\n{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
