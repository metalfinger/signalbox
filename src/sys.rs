//! Finding and starting programs when the app was opened outside a terminal.

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// Apps opened from Finder or Spotlight get a bare PATH, so add the usual install places.
/// Programs started by the app inherit the same PATH.
pub fn search_path() -> OsString {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for extra in [
        home.join(".local/bin"),
        home.join(".claude/local"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The full path of `name` on the search path, or just `name` if it isn't found.
pub fn find_program(name: &str) -> PathBuf {
    std::env::split_paths(&search_path())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// The locale for the app and the programs it starts when it was opened without one: macOS sets
/// no `LANG` for an app opened from Finder, Spotlight, the Dock or at login. Like a terminal, it
/// follows the person's region, in UTF-8. `None` when the environment already names a locale.
pub fn missing_locale() -> Option<String> {
    let named = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if named {
        return None;
    }
    let (language, country) = region();
    Some(utf8_locale(language.as_deref(), country.as_deref(), locale_exists))
}

/// `language_COUNTRY.UTF-8` if the system has it, otherwise `en_US.UTF-8`.
fn utf8_locale(language: Option<&str>, country: Option<&str>, exists: impl Fn(&str) -> bool) -> String {
    language
        .zip(country)
        .map(|(language, country)| format!("{language}_{country}.UTF-8"))
        .filter(|name| exists(name))
        .unwrap_or_else(|| "en_US.UTF-8".to_string())
}

/// Whether the C library has the locale `name`.
fn locale_exists(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    // Safety: a valid C string, and the locale is freed straight away.
    unsafe {
        let locale = libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut());
        if locale.is_null() {
            return false;
        }
        libc::freelocale(locale);
    }
    true
}

/// The language and country of the person's region (Language & Region in System Settings).
fn region() -> (Option<String>, Option<String>) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    let text = |string: *mut Object| -> Option<String> {
        if string.is_null() {
            return None;
        }
        // Safety: a live NSString, whose UTF-8 copy lasts as long as the autorelease pool.
        let utf8: *const libc::c_char = unsafe { msg_send![string, UTF8String] };
        (!utf8.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
    };
    // Safety: Foundation calls on live objects, in a pool drained once the strings are copied.
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let locale: *mut Object = msg_send![class!(NSLocale), currentLocale];
        let language: *mut Object = msg_send![locale, languageCode];
        let country: *mut Object = msg_send![locale, countryCode];
        let region = (text(language), text(country));
        let _: () = msg_send![pool, drain];
        region
    }
}

/// This boot's identity: macOS gives every start of the machine a new one.
pub fn boot_id() -> Option<String> {
    let mut buffer = [0u8; 64];
    let mut size = buffer.len();
    // Safety: the kernel writes at most `size` bytes, a NUL-terminated string.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    let id = std::ffi::CStr::from_bytes_until_nul(&buffer).ok()?.to_str().ok()?;
    (!id.is_empty()).then(|| id.to_string())
}

/// When the Mac last started, in seconds since 1970.
pub fn boot_time() -> Option<u64> {
    let mut boot = libc::timeval { tv_sec: 0, tv_usec: 0 };
    let mut size = std::mem::size_of::<libc::timeval>();
    // Safety: the kernel fills at most `size` bytes of a timeval.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&raw mut boot).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && boot.tv_sec > 0).then_some(boot.tv_sec as u64)
}

/// A command for `program` that starts with no signals blocked. A child inherits the signal
/// mask of the thread that starts it, and the app's background threads block every signal: a
/// tmux server started from one could never be stopped (kill-server, logging out) and never
/// cleared away its exited panes. Every program the app starts goes through here.
#[allow(clippy::disallowed_methods)]
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    // Safety: between fork and exec this only calls async-signal-safe functions.
    unsafe {
        command.pre_exec(|| {
            let mut none = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(none.as_mut_ptr());
            libc::pthread_sigmask(libc::SIG_SETMASK, none.as_ptr(), std::ptr::null_mut());
            Ok(())
        });
    }
    command
}

/// Where the app keeps its files: the window as you left it, preferences, workspace
/// snapshots, thumbnails and status line reports.
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".signalbox")
}

/// A second copy of the app for trying things out (`SIGNALBOX_DEV=1`): it saves nothing, posts
/// no alerts and leaves Login Items alone, so it can't disturb the copy in daily use.
pub fn dev_mode() -> bool {
    std::env::var_os("SIGNALBOX_DEV").is_some()
}

/// `SIGNALBOX_DEBUG=1`: say more, on stderr.
pub fn debug() -> bool {
    std::env::var_os("SIGNALBOX_DEBUG").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_locale_follows_the_region_when_the_system_has_it() {
        let known = |name: &str| name == "en_IN.UTF-8";
        assert_eq!(utf8_locale(Some("en"), Some("IN"), known), "en_IN.UTF-8");
        assert_eq!(utf8_locale(Some("xx"), Some("YY"), known), "en_US.UTF-8");
        assert_eq!(utf8_locale(Some("en"), None, known), "en_US.UTF-8");
        assert!(locale_exists("en_US.UTF-8"));
        assert!(!locale_exists("xx_YY.UTF-8"));
    }

    #[test]
    fn the_region_is_read() {
        let (language, _) = region();
        assert!(language.is_some_and(|language| !language.is_empty()));
    }
}
