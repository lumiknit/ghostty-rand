use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub(super) fn theme_dirs() -> Vec<PathBuf> {
    let mut paths = super::common_theme_dirs();
    if let Some(executable) = super::find_ghostty() {
        paths.extend(super::theme_dir_from_executable(&executable));
    }
    paths.push(PathBuf::from(
        "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
    ));
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(
            PathBuf::from(home).join("Applications/Ghostty.app/Contents/Resources/ghostty/themes"),
        );
    }
    paths
}

pub(super) fn launch(
    light: &str,
    dark: &str,
    font: Option<&String>,
    cwd: &Path,
    extra_args: &[OsString],
) -> io::Result<()> {
    let theme = format!("--theme=light:{light},dark:{dark}");
    let mut command = Command::new("open");
    command.args(["-na", "Ghostty.app", "--args"]);
    command.arg(theme);
    command.arg("--window-save-state=never");
    if let Some(font) = font {
        command.arg("--font-family=");
        command.arg(format!("--font-family={font}"));
    }
    command.arg(format!("--working-directory={}", cwd.to_string_lossy()));
    command.args(extra_args);
    command.spawn().map(|_| ())
}
