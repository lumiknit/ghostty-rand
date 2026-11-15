use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn theme_dirs() -> Vec<PathBuf> {
    let mut paths = super::common_theme_dirs();
    if let Some(executable) = super::find_ghostty() {
        paths.extend(super::theme_dir_from_executable(&executable));
    }

    for path in [
        "/usr/share/ghostty/themes",
        "/usr/local/share/ghostty/themes",
        "/usr/lib/ghostty/themes",
        "/usr/local/lib/ghostty/themes",
        "/opt/homebrew/share/ghostty/themes",
    ] {
        paths.push(PathBuf::from(path));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data_home).join("ghostty/themes"));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".local/share/ghostty/themes"));
    }
    paths
}

unsafe extern "C" {
    fn setsid() -> i32;
}

pub(super) fn launch(
    light: &str,
    dark: &str,
    font: Option<&String>,
    cwd: &Path,
    extra_args: &[OsString],
) -> io::Result<()> {
    let theme = format!("--theme=light:{light},dark:{dark}");
    let mut command = Command::new("ghostty");
    command.arg("--gtk-single-instance=false");
    command.arg(theme);
    command.arg("--window-save-state=never");
    if let Some(font) = font {
        command.arg("--font-family=");
        command.arg(format!("--font-family={font}"));
    }
    command.arg(format!("--working-directory={}", cwd.to_string_lossy()));
    command.args(extra_args);

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    unsafe {
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}
