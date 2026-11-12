//! Plattformsberoende små saker: användarmappar, öppna filer med systemet och
//! kontroll av om en process lever. Resten av koden ska inte behöva `cfg`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Mappnamnet under användarens data- och konfigurationsmapp.
/// Linux behåller gemener (som tidigare), macOS och Windows använder versal.
const APP_DIR: &str = if cfg!(any(target_os = "macos", windows)) { "Kladd" } else { "kladd" };

/// Kladds datamapp: `$XDG_DATA_HOME/kladd`, `~/Library/Application Support/Kladd` eller `%APPDATA%\Kladd`.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".local/share")).join(APP_DIR)
}

/// Kladds konfigurationsmapp: `$XDG_CONFIG_HOME/kladd`, `~/Library/Application Support/Kladd` eller `%APPDATA%\Kladd`.
#[allow(dead_code)] // används av stavningskontrollen när den byter till platform
pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join(APP_DIR)
}

/// Användarens hemmapp.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Gör att barnprocesser inte öppnar ett konsolfönster på Windows (inget på andra system).
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Öppnar en fil eller webbadress med systemets standardprogram.
pub fn open(target: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        // `start` är inbyggt i cmd; den tomma titeln hindrar att sökvägen tolkas som fönstertitel.
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(target);
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    quiet(&mut cmd).spawn().map(|_| ())
}

/// Lever processen med det här numret?
#[cfg(target_os = "linux")]
pub fn pid_alive(pid: u32) -> bool {
    // En zombie (avslutad men ännu inte inhämtad av sin förälder) räknas som död.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat.rsplit(')').next().and_then(|r| r.trim().chars().next()) != Some('Z'),
        Err(_) => false,
    }
}

/// Lever processen med det här numret? (macOS och andra Unix saknar /proc.)
#[cfg(all(unix, not(target_os = "linux")))]
pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 skickar inget men kontrollerar att processen finns. EPERM betyder att den
    // finns men tillhör någon annan, vilket också räknas som levande.
    // SAFETY: kill med signal 0 har inga biverkningar.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Lever processen med det här numret?
#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: handtaget används bara här och stängs innan vi lämnar funktionen.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_end_with_app_name() {
        assert!(data_dir().ends_with(APP_DIR));
        assert!(config_dir().ends_with(APP_DIR));
    }

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id()));
    }
}
