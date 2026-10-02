//! Opening the app at login, so it keeps watching sessions without being started by hand.
//! It registers once; after that the setting is the person's, in System Settings > General >
//! Login Items, and the app never adds itself back.

use std::path::PathBuf;

use objc::runtime::{BOOL, Class, NO, Object};
use objc::{class, msg_send, sel, sel_impl};

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

type Id = *mut Object;

fn marker() -> PathBuf {
    crate::sys::data_dir().join("login-item-added")
}

/// Adds the app to Login Items the first time it runs from its bundle.
pub fn open_at_login_once() {
    if crate::sys::dev_mode() || marker().exists() {
        return;
    }
    let Some(service) = Class::get("SMAppService") else {
        return;
    };
    unsafe {
        let bundle: Id = msg_send![class!(NSBundle), mainBundle];
        let identifier: Id = msg_send![bundle, bundleIdentifier];
        if identifier.is_null() {
            return;
        }
        let app: Id = msg_send![service, mainAppService];
        let mut error: Id = std::ptr::null_mut();
        let added: BOOL = msg_send![app, registerAndReturnError: &mut error];
        if added != NO {
            let _ = std::fs::write(marker(), "Signalbox added itself to Login Items once.\n");
        } else if crate::sys::debug() {
            let description: Id = if error.is_null() {
                std::ptr::null_mut()
            } else {
                msg_send![error, localizedDescription]
            };
            let reason = if description.is_null() {
                String::new()
            } else {
                let utf8: *const std::os::raw::c_char = msg_send![description, UTF8String];
                std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
            };
            eprintln!("[login] couldn't add to Login Items: {reason}");
        }
    }
}
