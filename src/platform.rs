use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "platform/unix.rs"]
mod imp;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("ghostty-rand supports macOS and Linux only");

pub fn theme_dirs() -> Vec<PathBuf> {
    imp::theme_dirs()
}

pub fn launch(
    light: &str,
    dark: &str,
    font: Option<&String>,
    cwd: &Path,
    extra_args: &[OsString],
) -> io::Result<()> {
    imp::launch(light, dark, font, cwd, extra_args)
}

fn common_theme_dirs() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(resources) = std::env::var_os("GHOSTTY_RESOURCES_DIR") {
        let resources = PathBuf::from(resources);
        paths.push(resources.join("themes"));
        paths.push(resources.join("ghostty").join("themes"));
        paths.push(resources);
    }
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(config_home).join("ghostty/themes"));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".config/ghostty/themes"));
    }
    paths
}

fn theme_dir_from_executable(executable: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(executable) = executable.canonicalize() else {
        return paths;
    };

    // Ghostty on macOS keeps its theme files in the app's resource bundle.
    for ancestor in executable.ancestors() {
        if ancestor.file_name().is_some_and(|name| name == "MacOS") {
            if let Some(contents) = ancestor.parent() {
                paths.push(contents.join("Resources/ghostty/themes"));
            }
        }
    }

    // Common Linux and manually unpacked installations place resources beside
    // the executable tree (for example, /usr/bin/ghostty and /usr/share/ghostty).
    if let Some(bin_dir) = executable.parent() {
        if let Some(prefix) = bin_dir.parent() {
            paths.push(prefix.join("share/ghostty/themes"));
            paths.push(prefix.join("lib/ghostty/themes"));
            paths.push(prefix.join("lib/ghostty/resources/themes"));
        }
        paths.push(bin_dir.join("themes"));
        paths.push(bin_dir.join("../share/ghostty/themes"));
    }
    paths
}

fn find_ghostty() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;

    for directory in std::env::split_paths(&path) {
        let executable = directory.join("ghostty");
        if executable.is_file() {
            return Some(executable);
        }
    }
    None
}
