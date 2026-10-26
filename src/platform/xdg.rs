//! freedesktop.org integration: per-connection app launcher entries and auto-connect
//! at login (XDG autostart). Both are `.desktop` files that run `--connect <target>`.

use crate::config::APP_ID;
use crate::launcher::{LaunchEntry, LaunchTarget};
use std::io;
use std::path::{Path, PathBuf};

/// App launcher entry for `entry`, written to `~/.local/share/applications`.
pub fn install_launcher(entry: &LaunchEntry) -> io::Result<PathBuf> {
    let path = launcher_path(&entry.target)?;
    write_entry(&path, &desktop_entry(entry, &exec_path(), false))?;
    Ok(path)
}

pub fn remove_launcher(target: &LaunchTarget) -> io::Result<()> {
    crate::config::storage::remove_if_exists(&launcher_path(target)?)
}

#[must_use]
pub fn has_launcher(target: &LaunchTarget) -> bool {
    launcher_path(target).is_ok_and(|p| p.exists())
}

/// Connects to `entry` when the user logs in (or stops doing so).
pub fn set_autostart(entry: &LaunchEntry, enabled: bool) -> io::Result<()> {
    let path = autostart_path(&entry.target)?;
    if enabled {
        write_entry(&path, &desktop_entry(entry, &exec_path(), true))
    } else {
        crate::config::storage::remove_if_exists(&path)
    }
}

#[must_use]
pub fn has_autostart(target: &LaunchTarget) -> bool {
    autostart_path(target).is_ok_and(|p| p.exists())
}

/// Removes the launcher and autostart entries of a connection that no longer exists.
pub fn remove_desktop_integration(target: &LaunchTarget) {
    for path in [launcher_path(target), autostart_path(target)]
        .into_iter()
        .flatten()
    {
        if let Err(e) = crate::config::storage::remove_if_exists(&path) {
            log::warn!("Could not remove {}: {e}", path.display());
        }
    }
}

fn file_name(target: &LaunchTarget) -> String {
    format!("{APP_ID}.connection-{}.desktop", target.file_key())
}

fn launcher_path(target: &LaunchTarget) -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?;
    Ok(base.data_dir().join("applications").join(file_name(target)))
}

fn autostart_path(target: &LaunchTarget) -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?;
    Ok(base.config_dir().join("autostart").join(file_name(target)))
}

fn exec_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("windows-app-linux"))
}

fn write_entry(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// `.desktop` file text that runs `exec --connect <target>`.
#[must_use]
pub fn desktop_entry(entry: &LaunchEntry, exec: &Path, autostart: bool) -> String {
    let target = entry.target.to_string();
    let exec_line = [
        exec.to_string_lossy().as_ref(),
        "--connect",
        target.as_str(),
    ]
    .iter()
    .map(|arg| quote_exec_arg(arg))
    .collect::<Vec<_>>()
    .join(" ");
    let mut text = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name={name}\n\
         Comment={comment}\n\
         Exec={exec}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Network;RemoteAccess;\n\
         StartupWMClass={APP_ID}\n\
         X-WindowsApp-Target={target}\n",
        name = escape_value(&entry.name),
        comment = escape_value(&entry.description),
        exec = escape_value(&exec_line),
    );
    if autostart {
        text.push_str(
            "NoDisplay=true\nX-GNOME-Autostart-enabled=true\nX-GNOME-Autostart-Delay=5\n",
        );
    }
    text
}

/// Escapes a string value (desktop entry spec, "Possible value types").
fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Quotes one `Exec` argument ("The Exec key"); `escape_value` is applied afterwards.
fn quote_exec_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:{}+,@=".contains(c));
    if plain {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_exec_and_escapes_values() {
        let entry = LaunchEntry {
            target: LaunchTarget::RemotePc("abc-1".into()),
            name: "Lab\nPC".into(),
            description: "Remote PC · lab".into(),
        };
        let text = desktop_entry(&entry, Path::new("/opt/My Apps/w$app"), true);
        assert!(text.contains("Name=Lab\\nPC\n"));
        assert!(text.contains("Exec=\"/opt/My Apps/w\\\\$app\" --connect remote:abc-1\n"));
        assert!(text.contains("X-GNOME-Autostart-enabled=true"));
        assert!(text.contains(&format!("Icon={APP_ID}\n")));

        let plain = desktop_entry(&entry, Path::new("/usr/bin/windows-app-linux"), false);
        assert!(plain.contains("Exec=/usr/bin/windows-app-linux --connect remote:abc-1\n"));
        assert!(!plain.contains("Autostart"));
        assert_eq!(
            file_name(&entry.target),
            format!("{APP_ID}.connection-{}.desktop", entry.target.file_key())
        );
    }
}
