use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub lang: Lang,
    pub unlock: bool,
    pub force: bool,
    pub pinned: bool,
}

#[derive(Debug)]
enum ReadError {
    Missing,
    Io(String),
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: Lang::Zh,
            unlock: true,
            force: false,
            pinned: false,
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| config_path_for(&exe))
        .unwrap_or_else(|| PathBuf::from("ffrm.cfg"))
}

fn config_path_for(exe: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(path) = macos_app_config(exe) {
        return path;
    }
    exe.parent()
        .map(|dir| dir.join("ffrm.cfg"))
        .unwrap_or_else(|| PathBuf::from("ffrm.cfg"))
}

#[cfg(target_os = "macos")]
fn macos_app_config(exe: &Path) -> Option<PathBuf> {
    if !is_app_executable(exe) {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("ffrm")
            .join("ffrm.cfg"),
    )
}

#[cfg(target_os = "macos")]
fn is_app_executable(exe: &Path) -> bool {
    let Some(macos) = exe.parent() else {
        return false;
    };
    if macos.file_name().is_none_or(|name| name != "MacOS") {
        return false;
    }
    let Some(contents) = macos.parent() else {
        return false;
    };
    if contents.file_name().is_none_or(|name| name != "Contents") {
        return false;
    }
    contents
        .parent()
        .and_then(|app| app.extension())
        .is_some_and(|ext| ext == "app")
}

#[cfg(target_os = "macos")]
fn adopt_legacy_config(dest: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if !is_app_executable(&exe) {
        return;
    }
    let Some(legacy) = exe.parent().map(|dir| dir.join("ffrm.cfg")) else {
        return;
    };
    if legacy == dest || !legacy.is_file() {
        return;
    }
    if dest.is_file() {
        let _ = fs::remove_file(&legacy);
        return;
    }
    if let Some(parent) = dest.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if fs::rename(&legacy, dest).is_err() && fs::copy(&legacy, dest).is_ok() {
        let _ = fs::remove_file(&legacy);
    }
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        let path = config_path();
        #[cfg(target_os = "macos")]
        adopt_legacy_config(&path);
        match Self::load_from(&path) {
            Ok(config) => (config, None),
            Err(ReadError::Missing) => {
                let config = Self::default();
                let error = config.save_to(&path).err();
                (config, error)
            }
            Err(ReadError::Io(error)) => {
                (Self::default(), Some(format!("无法读取配置文件: {error}")))
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&config_path())
    }

    fn load_from(path: &Path) -> Result<Self, ReadError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ReadError::Missing)
            }
            Err(error) => return Err(ReadError::Io(error.to_string())),
        };
        Ok(Self::decode(&text))
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| self.write_error(path, &error))?;
            }
        }
        fs::write(path, self.encode()).map_err(|error| self.write_error(path, &error))
    }

    fn encode(&self) -> String {
        format!(
            "\u{feff}# ffrm {version}\n# lang 为 zh 或 en。unlock、force、pinned 为 true 或 false。\nlang={lang}\nunlock={unlock}\nforce={force}\npinned={pinned}\n",
            version = env!("CARGO_PKG_VERSION"),
            lang = match self.lang {
                Lang::Zh => "zh",
                Lang::En => "en",
            },
            unlock = self.unlock,
            force = self.force,
            pinned = self.pinned,
        )
    }

    fn decode(text: &str) -> Self {
        let mut config = Self::default();
        let text = text.trim_start_matches('\u{feff}');
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "lang" => {
                    config.lang = match value.to_ascii_lowercase().as_str() {
                        "en" | "english" => Lang::En,
                        _ => Lang::Zh,
                    };
                }
                "unlock" => {
                    if let Some(value) = parse_bool(value) {
                        config.unlock = value;
                    }
                }
                "force" => {
                    if let Some(value) = parse_bool(value) {
                        config.force = value;
                    }
                }
                "pinned" => {
                    if let Some(value) = parse_bool(value) {
                        config.pinned = value;
                    }
                }
                _ => {}
            }
        }
        config
    }

    fn write_error(&self, path: &Path, error: &io::Error) -> String {
        match self.lang {
            Lang::Zh => format!("无法写入 {}: {error}", path.display()),
            Lang::En => format!("Could not write {}: {error}", path.display()),
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_unknown_values() {
        let config = Config::decode("# comment\nlang=nope\nunlock=maybe\nextra=1\n");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn portable_config_stays_beside_the_executable() {
        let exe = Path::new("/Users/me/Desktop/ffrm");
        assert_eq!(
            config_path_for(exe),
            Path::new("/Users/me/Desktop/ffrm.cfg")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_config_lives_outside_the_bundle() {
        let exe = Path::new("/Applications/ffrm.app/Contents/MacOS/ffrm");
        let path = config_path_for(exe);
        assert!(path.ends_with("Library/Application Support/ffrm/ffrm.cfg"));
        assert!(!path.starts_with("/Applications/ffrm.app"));
    }

    #[test]
    fn roundtrip_file() {
        let dir = std::env::temp_dir().join(format!("ffrm-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ffrm.cfg");
        let config = Config {
            lang: Lang::En,
            unlock: false,
            force: true,
            pinned: true,
        };
        config.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, config);
        let _ = fs::remove_dir_all(&dir);
    }
}
