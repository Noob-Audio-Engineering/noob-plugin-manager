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

/// Start this program again with administrator rights, running `args`.
///
/// Windows only: on macOS everything installs into the user's own folders, so
/// there is nothing that elevation would unlock and nothing to ask for.
#[cfg(windows)]
pub fn relaunch(args: &[String]) -> Result<(), String> {
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
        _ => Ok(()),
    }
}

#[cfg(not(windows))]
pub fn relaunch(_args: &[String]) -> Result<(), String> {
    Err("this system installs into your own folders, so administrator is never needed".into())
}
