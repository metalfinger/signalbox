//! Getting your attention when a session needs you: a count on the Dock icon, a macOS
//! notification with a sound, and a click on it opens that session's window.
//!
//! Notifications need the app bundle (Signalbox.app); run outside it, only the badge and a
//! bounce are used.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use block::{Block, ConcreteBlock};
use objc::declare::ClassDecl;
use objc::rc::autoreleasepool;
use objc::runtime::{BOOL, Class, NO, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

type Id = *mut Object;
const NIL: Id = std::ptr::null_mut();

/// What a notification is about. Each kind has its own sound, from macOS's own set
/// (`macos/install.sh` copies them into the app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A Claude session is waiting on you: a question, or a permission.
    NeedsYou,
    /// A Claude session finished its turn.
    Finished,
    /// Something to know, like a workspace rebuilt after a restart.
    Info,
}

impl Kind {
    pub fn sound(self) -> &'static str {
        match self {
            Kind::NeedsYou => "Glass",
            Kind::Finished => "Pop",
            Kind::Info => "Purr",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Kind::NeedsYou => "needs-you",
            Kind::Finished => "finished",
            Kind::Info => "info",
        }
    }
}

/// The tmux window on screen in the active tab of the frontmost Signalbox window.
static ON_SCREEN: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

pub fn set_on_screen(place: Option<(String, String)>) {
    *ON_SCREEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = place;
}

/// Whether you're looking at this window right now: Signalbox in front, showing it.
pub fn watching(session: &str, window_id: &str) -> bool {
    let active: BOOL = unsafe { msg_send![app(), isActive] };
    let on_screen = ON_SCREEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    active != NO && on_screen.as_ref().is_some_and(|(s, w)| s == session && w == window_id)
}

/// A click on a notification: show this tmux window.
pub struct Click {
    pub session: String,
    pub window_id: String,
}

static CLICKS: OnceLock<async_channel::Sender<Click>> = OnceLock::new();
static ALLOWED: AtomicBool = AtomicBool::new(false);

unsafe fn ns_string(text: &str) -> Id {
    let text = CString::new(text.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

unsafe fn rust_string(string: Id) -> Option<String> {
    if string.is_null() {
        return None;
    }
    let utf8: *const c_char = unsafe { msg_send![string, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
}

fn app() -> Id {
    unsafe { msg_send![class!(NSApplication), sharedApplication] }
}

/// The number of sessions waiting on you, on the Dock icon (nothing at zero).
pub fn set_badge(count: usize) {
    autoreleasepool(|| unsafe {
        let tile: Id = msg_send![app(), dockTile];
        let label = if count == 0 { NIL } else { ns_string(&count.to_string()) };
        let _: () = msg_send![tile, setBadgeLabel: label];
    });
}

/// Bounces the Dock icon once, if the app isn't in front.
pub fn bounce() {
    unsafe {
        let _: isize = msg_send![app(), requestUserAttention: 10isize];
    }
}

/// Plays a system sound by name (e.g. "Glass"), for when notifications are off.
fn play(name: &str) {
    autoreleasepool(|| unsafe {
        let sound: Id = msg_send![class!(NSSound), soundNamed: ns_string(name)];
        if !sound.is_null() {
            let _: BOOL = msg_send![sound, play];
        }
    });
}

fn notification_center() -> Option<Id> {
    unsafe {
        let bundle: Id = msg_send![class!(NSBundle), mainBundle];
        let identifier: Id = msg_send![bundle, bundleIdentifier];
        if identifier.is_null() {
            return None;
        }
    }
    let center = Class::get("UNUserNotificationCenter")?;
    Some(unsafe { msg_send![center, currentNotificationCenter] })
}

/// Asks for permission to notify (macOS asks the person once), and returns clicks on notifications.
pub fn start() -> async_channel::Receiver<Click> {
    let (tx, rx) = async_channel::unbounded();
    if CLICKS.set(tx).is_err() {
        return rx;
    }
    if crate::sys::debug() {
        eprintln!("[alerts] notification center: {}", notification_center().is_some());
    }
    if let Some(center) = notification_center() {
        autoreleasepool(|| unsafe {
            // Lives as long as the app: the center keeps only a weak reference.
            let delegate: Id = msg_send![delegate_class(), new];
            let _: () = msg_send![center, setDelegate: delegate];
            let answered = ConcreteBlock::new(|granted: BOOL, error: Id| {
                ALLOWED.store(granted != NO, Ordering::Relaxed);
                if crate::sys::debug() {
                    let reason = if error.is_null() {
                        None
                    } else {
                        let description: Id = msg_send![error, localizedDescription];
                        rust_string(description)
                    };
                    eprintln!("[alerts] notifications allowed: {}, {reason:?}", granted != NO);
                }
            })
            .copy();
            // Badge | sound | alert.
            let options: usize = (1 << 0) | (1 << 1) | (1 << 2);
            let _: () = msg_send![center, requestAuthorizationWithOptions: options completionHandler: answered];
        });
    }
    rx
}

/// Tells you about a Claude session in a tmux window. A notification of the same kind for the
/// same window replaces the earlier one.
pub fn notify(kind: Kind, title: &str, subtitle: &str, body: &str, session: &str, window_id: &str) {
    let center = notification_center().filter(|_| ALLOWED.load(Ordering::Relaxed));
    let Some(center) = center else {
        play(kind.sound());
        if kind == Kind::NeedsYou {
            bounce();
        }
        return;
    };
    autoreleasepool(|| unsafe {
        let content: Id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setSubtitle: ns_string(subtitle)];
        let _: () = msg_send![content, setBody: ns_string(body)];
        let _: () = msg_send![content, setThreadIdentifier: ns_string(session)];
        let _: () = msg_send![content, setSound: sound(kind)];
        let keys = [ns_string("session"), ns_string("window")];
        let values = [ns_string(session), ns_string(window_id)];
        let info: Id = msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: 2usize];
        let _: () = msg_send![content, setUserInfo: info];
        let request: Id = msg_send![class!(UNNotificationRequest),
            requestWithIdentifier: ns_string(&identifier(kind, session, window_id)) content: content trigger: NIL];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: NIL];
        let _: () = msg_send![content, release];
    });
    if kind == Kind::NeedsYou {
        bounce();
    }
}

/// The kind's sound, from the app's resources.
unsafe fn sound(kind: Kind) -> Id {
    unsafe { msg_send![class!(UNNotificationSound), soundNamed: ns_string(&format!("{}.aiff", kind.sound()))] }
}

/// A notification that only tells you something; clicking it brings the app forward.
pub fn notify_info(title: &str, body: &str) {
    let Some(center) = notification_center().filter(|_| ALLOWED.load(Ordering::Relaxed)) else {
        return;
    };
    autoreleasepool(|| unsafe {
        let content: Id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setBody: ns_string(body)];
        let _: () = msg_send![content, setSound: sound(Kind::Info)];
        let request: Id = msg_send![class!(UNNotificationRequest),
            requestWithIdentifier: ns_string(&format!("{}:{title}", Kind::Info.tag())) content: content trigger: NIL];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: NIL];
        let _: () = msg_send![content, release];
    });
}

/// Removes a window's notification of this kind once it no longer applies.
pub fn clear(kind: Kind, session: &str, window_id: &str) {
    let Some(center) = notification_center() else { return };
    autoreleasepool(|| unsafe {
        let ids: Id = msg_send![class!(NSArray), arrayWithObject: ns_string(&identifier(kind, session, window_id))];
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: ids];
    });
}

fn identifier(kind: Kind, session: &str, window_id: &str) -> String {
    format!("{}:{session}:{window_id}", kind.tag())
}

fn delegate_class() -> &'static Class {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("SignalboxNotificationDelegate", class!(NSObject)).expect("class name is free");
        unsafe {
            decl.add_method(
                sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
                clicked as extern "C" fn(&Object, Sel, Id, Id, Id),
            );
            decl.add_method(
                sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
                presenting as extern "C" fn(&Object, Sel, Id, Id, Id),
            );
        }
        decl.register();
    });
    Class::get("SignalboxNotificationDelegate").expect("registered above")
}

/// A click: send the session and window to the app, then tell macOS we're done.
extern "C" fn clicked(_this: &Object, _sel: Sel, _center: Id, response: Id, handler: Id) {
    unsafe {
        let notification: Id = msg_send![response, notification];
        let request: Id = msg_send![notification, request];
        let content: Id = msg_send![request, content];
        let info: Id = msg_send![content, userInfo];
        let session = rust_string(msg_send![info, objectForKey: ns_string("session")]);
        let window_id = rust_string(msg_send![info, objectForKey: ns_string("window")]);
        if let (Some(session), Some(window_id), Some(clicks)) = (session, window_id, CLICKS.get()) {
            let _ = clicks.try_send(Click { session, window_id });
        }
        if !handler.is_null() {
            (*(handler as *mut Block<(), ()>)).call(());
        }
    }
}

/// Show the banner and play the sound even when the app is in front.
extern "C" fn presenting(_this: &Object, _sel: Sel, _center: Id, _notification: Id, handler: Id) {
    // Sound | list | banner.
    let options: usize = (1 << 1) | (1 << 3) | (1 << 4);
    if !handler.is_null() {
        unsafe { (*(handler as *mut Block<(usize,), ()>)).call((options,)) };
    }
}
