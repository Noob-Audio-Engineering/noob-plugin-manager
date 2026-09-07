//! Asking for administrator, and knowing when it would not help.
//!
//! "Access is denied" has two causes here and they want opposite answers.
//!
//! **A directory that cannot be written** is a permissions problem, and
//! elevation fixes it.
//!
//! **A file a host is holding open** is not, and elevation does nothing for
//! it --- an administrator cannot replace a loaded library either. Offering a
//! prompt that then fails anyway is worse than not offering one, because it
//! teaches people to click through the next one.
//!
//! So the failure is classified before anything is offered.

use std::path::Path;

/// Why an install could not write where it meant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The directory refuses writes. Elevation is the answer.
    Directory,
    /// The directory is fine and the file itself is held. Elevation is not.
    FileInUse,
    /// Not a permission problem at all.
    Other,
}

/// Classify a failed write by asking the directory whether *it* is the
/// problem, rather than by reading the message.
pub fn classify(target: &Path) -> Denial {
    let Some(dir) = target.parent() else {
        return Denial::Other;
    };
    let probe = dir.join(".noob-elevate-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            // We can write here, so the directory is not what refused --- the
            // file itself is held by something.
            Denial::FileInUse
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Denial::Directory,
        Err(_) => Denial::Other,
    }
}

/// Start this program again with administrator rights, running `args`, and
/// return whatever it printed on stdout.
///
/// On Windows this raises the consent prompt; on macOS, the password one.
///
/// **Windows returns nothing.** `ShellExecuteW` starts the process and does
/// not wait for it or give a handle to its output, so there is no result to
/// read --- and none is needed: an elevated process on Windows keeps the same
/// `LOCALAPPDATA`, so it writes the record to the same file the unelevated one
/// reads. macOS is where the two disagree, and macOS is where the result is
/// used.
#[cfg(windows)]
pub fn relaunch(args: &[String]) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let exe = wide(&exe.to_string_lossy());
    let params = wide(&args.join(" "));
    let verb = wide("runas");

    // `ShellExecuteW` with the `runas` verb is what raises the consent
    // prompt; there is no way to elevate a process already running.
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            lpOperation: *const u16,
            lpFile: *const u16,
            lpParameters: *const u16,
            lpDirectory: *const u16,
            nShowCmd: i32,
        ) -> isize;
    }

    // Anything at or below 32 is a failure code, and 5 is the user saying no.
    let r = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            exe.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            1,
        )
    };
    match r {
        5 => Err("the request for administrator was declined".into()),
        n if n <= 32 => Err(format!("could not ask for administrator (code {n})")),
        _ => Ok(String::new()),
    }
}

/// Start this program again with administrator rights, running `args`, and
/// return whatever it printed on stdout.
///
/// `osascript` is how a macOS program asks: `with administrator privileges`
/// shows the system's own password panel, and what it runs, runs as root.
/// There is no way to raise the rights of a process that is already going, on
/// either system --- both of these start a second copy and let the first one
/// finish.
///
/// **The stdout is the point, not a courtesy.** The elevated copy runs as
/// root, and where the install record lives is resolved against `$HOME`, so
/// that copy cannot write the record the user's own window reads --- it would
/// either put it in root's home or leave the user's file owned by root and
/// unwritable ever after. It prints the record instead, `do shell script`
/// returns it here, and the caller --- still the user --- saves it.
///
/// The command is assembled with the executable path quoted, because a person
/// may well have put this program somewhere with a space in the name and the
/// shell that `osascript` spawns would otherwise read it as two words.
#[cfg(target_os = "macos")]
pub fn relaunch(args: &[String]) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let mut cmd = shell_quote(&exe.to_string_lossy());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    // The whole shell command becomes an AppleScript string, so its quotes
    // and backslashes have to survive a second round of escaping.
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        cmd.replace(BACKSLASH, "\\\\").replace(QUOTE, "\\\"")
    );
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("could not ask for administrator: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // -128 is the person pressing Cancel, which is not a fault to report as
    // one.
    if err.contains("-128") || err.to_lowercase().contains("user canceled") {
        return Err("the request for administrator was declined".into());
    }
    Err(format!("could not ask for administrator: {}", err.trim()))
}

#[cfg(target_os = "macos")]
const BACKSLASH: char = '\\';
#[cfg(target_os = "macos")]
const QUOTE: char = '"';

/// Wrap a word so a shell reads it as one word, whatever is in it.
#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    // Single quotes take everything literally; the only thing that cannot
    // appear inside them is a single quote, which is closed, escaped and
    // reopened in the usual way.
    format!("{Q}{}{Q}", s.replace("'", r"'\''"), Q = "'")
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn relaunch(_args: &[String]) -> Result<String, String> {
    Err("this system installs into your own folders, so administrator is never needed".into())
}
