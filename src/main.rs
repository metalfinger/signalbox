mod alerts;
mod live;
mod login;
mod media;
mod media_panel;
mod shell;
mod sidebar;
mod snapshots;
mod start;
mod status;
mod store;
mod switcher;
mod sys;
mod terminal;
mod theme;
mod tmux;
mod tmux_tab;
mod transcript;
mod ui;
mod watch;
mod watcher;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, Focusable, Global, KeyBinding, Menu, MenuItem, SystemMenuType,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, point, prelude::*, px,
    size,
};

use shell::{Opening, Shell};
use store::{SavedTab, WindowState};

actions!(signalbox, [Quit, NewWindow, SaveWorkspace, RestoreWorkspace]);

/// The app's window and the shell in it. The window's root is the kit's `Root`, which
/// draws its dialogs and notifications over the shell.
#[derive(Clone)]
struct MainWindow {
    window: AnyWindowHandle,
    shell: Entity<Shell>,
}

impl MainWindow {
    /// Runs `update` on the shell inside its window; fails once the window is closed.
    fn update<R>(
        &self,
        cx: &mut App,
        update: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) -> R,
    ) -> anyhow::Result<R> {
        let shell = self.shell.clone();
        self.window
            .update(cx, |_, window, cx| shell.update(cx, |shell, cx| update(shell, window, cx)))
    }
}

/// The main window, while it is open. The app keeps running when it's closed.
struct OpenWindow(Option<MainWindow>);

impl Global for OpenWindow {}

/// Opened with nothing to show: the tabs, sidebar and window from last time, minus tmux
/// sessions that are gone.
/// The window as it was left: its tabs, the active one, the sidebar and where the window was.
struct LastTime {
    openings: Vec<Opening>,
    active: usize,
    sidebar: bool,
    sidebar_width: Option<f32>,
    bounds: Option<WindowBounds>,
}

fn last_time(cx: &App) -> Option<LastTime> {
    let saved = store::load_window()?;
    // While tmux can't be read every tab stays; each attaches once its session answers.
    let running: Option<Vec<String>> = tmux::sessions()
        .ok()
        .map(|sessions| sessions.into_iter().map(|session| session.name).collect());
    // Sessions about to be rebuilt after a restart keep their tabs, which attach as they come back.
    let rebuilding: Vec<String> = watcher::pending_rebuild()
        .map(|(_, workspace)| workspace.sessions.into_iter().map(|session| session.name).collect())
        .unwrap_or_default();
    let mut openings = Vec::new();
    let mut active = 0;
    for (ix, tab) in saved.tabs.into_iter().enumerate() {
        let opening = match tab {
            SavedTab::Start => Some(Opening::Start),
            SavedTab::Tmux { session, window } => {
                running
                    .as_ref()
                    .is_none_or(|running| running.contains(&session) || rebuilding.contains(&session))
                    .then_some(Opening::Tmux { session, window })
            }
        };
        if let Some(opening) = opening {
            if ix == saved.active {
                active = openings.len();
            }
            openings.push(opening);
        }
    }
    let bounds = saved.bounds.and_then(|b| {
        let bounds = Bounds::new(point(px(b.x), px(b.y)), size(px(b.width), px(b.height)));
        // Only where a display still is: a window saved on a monitor that's gone opens centered.
        let visible = cx.displays().iter().any(|display| {
            let screen = display.bounds();
            screen.contains(&point(bounds.origin.x + px(40.), bounds.origin.y + px(20.)))
        });
        visible.then_some(match b.state {
            WindowState::Windowed => WindowBounds::Windowed(bounds),
            WindowState::Maximized => WindowBounds::Maximized(bounds),
            WindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
        })
    });
    Some(LastTime {
        openings,
        active,
        sidebar: saved.sidebar,
        sidebar_width: saved.sidebar_width,
        bounds,
    })
}

fn open_main_window(openings: Vec<Opening>, cx: &mut App) -> Option<MainWindow> {
    let restored = if openings.is_empty() { last_time(cx) } else { None };
    let (openings, view, bounds) = match restored {
        Some(last) => (last.openings, Some((last.active, last.sidebar, last.sidebar_width)), last.bounds),
        None => (openings, None, None),
    };
    let bounds =
        bounds.unwrap_or_else(|| WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(800.)), cx)));
    // Our own title bar (tabs beside the window buttons) over a blurred background: the
    // sidebar, title bar and status bar are glass.
    let options = WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(TitlebarOptions {
            title: Some("Signalbox".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(13.))),
        }),
        window_background: WindowBackgroundAppearance::Blurred,
        window_min_size: Some(size(px(720.), px(420.))),
        ..gpui_kit::component::TitleBar::window_options()
    };
    let (window, shell) = gpui_kit::open_window(options, cx, |window, cx| {
        let shell = cx.new(|cx| Shell::new(openings, window, cx));
        let handle = shell.focus_handle(cx);
        window.focus(&handle, cx);
        shell
    })
    .ok()?;
    let main = MainWindow { window, shell };
    if let Some((active, sidebar, width)) = view {
        let _ = main.update(cx, |shell, window, cx| shell.restore_view(active, sidebar, width, window, cx));
    }
    cx.set_global(OpenWindow(Some(main.clone())));
    cx.activate(true);
    Some(main)
}

/// The window, brought forward, or a new one if it was closed.
fn main_window(cx: &mut App) -> Option<MainWindow> {
    if let Some(main) = cx.try_global::<OpenWindow>().and_then(|open| open.0.clone())
        && main.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        cx.activate(true);
        return Some(main);
    }
    open_main_window(Vec::new(), cx)
}

fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Signalbox".into(),
            disabled: false,
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Signalbox", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("New Tab", shell::NewTab),
                MenuItem::separator(),
                MenuItem::action("Save Workspace Now", SaveWorkspace),
                MenuItem::action("Restore Workspace", RestoreWorkspace),
                MenuItem::separator(),
                MenuItem::action("Close Tab", shell::CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Copy", terminal::Copy),
                MenuItem::action("Paste", terminal::Paste),
                MenuItem::separator(),
                MenuItem::action("Find…", tmux_tab::Find),
            ],
        },
        Menu {
            name: "Go".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Go to Window…", shell::GoToWindow),
                MenuItem::action("Next Session That Needs You", shell::NextNeedingYou),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Sidebar", shell::ToggleSidebar),
                MenuItem::action("Side Panel", tmux_tab::TogglePanel),
                MenuItem::separator(),
                MenuItem::action("Bigger", terminal::Bigger),
                MenuItem::action("Smaller", terminal::Smaller),
                MenuItem::action("Actual Size", terminal::ActualSize),
            ],
        },
    ]);
}

/// `signalbox [--tmux SESSION]... [--start] [--go-to]`. With nothing to open, the window opens as
/// you left it. `--go-to` opens straight into the Cmd-K palette, for a global hotkey.
fn parse_args() -> (Vec<Opening>, bool) {
    let mut openings = Vec::new();
    let mut go_to = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--go-to" {
            go_to = true;
        } else if arg == "--start" {
            openings.push(Opening::Start);
        } else if arg == "--tmux"
            && let Some(session) = args.next()
        {
            openings.push(Opening::Tmux { session, window: None });
        }
    }
    (openings, go_to)
}

/// Opened from Finder, Spotlight or the Dock there is no terminal, so errors go to
/// ~/.signalbox/last-run.log (the `signalbox` command already sends them there).
fn keep_errors_in_a_log() {
    use std::io::IsTerminal;
    use std::os::fd::AsRawFd;
    if std::io::stderr().is_terminal() || sys::debug() {
        return;
    }
    let dir = sys::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(file) = std::fs::File::create(dir.join("last-run.log")) {
        // Safety: both descriptors are valid, and nothing has written to stderr yet.
        unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
    }
}

fn main() {
    // Opened from inside tmux, the app would otherwise refuse to attach its own tmux clients,
    // and Claude workers would claim the launching pane as theirs. Opened from Finder,
    // Spotlight, the Dock or at login, it has no locale, and nor would anything it starts: the
    // sessions it rebuilds after a restart would run without UTF-8.
    let locale = sys::missing_locale();
    // Safety: the app hasn't started any threads, so none reads the environment meanwhile.
    unsafe {
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");
        if let Some(locale) = &locale {
            std::env::set_var("LANG", locale);
        }
    }
    // Programs inherit the signal mask of the thread that starts them. The terminal starts its
    // programs from this thread, so it blocks nothing, whatever opened the app (background
    // threads go through `sys::command`).
    // Safety: an empty set, applied to this thread only.
    unsafe {
        let mut none = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(none.as_mut_ptr());
        libc::pthread_sigmask(libc::SIG_SETMASK, none.as_ptr(), std::ptr::null_mut());
    }
    keep_errors_in_a_log();
    let (openings, go_to) = parse_args();
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // The app keeps watching with no window open; clicking its Dock icon brings one back.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_main_window(Vec::new(), cx);
        }
    });
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        theme::use_dark_appearance();
        theme::load_fonts(cx);
        theme::apply(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        shell::bind_keys(cx);
        terminal::init(cx);
        tmux_tab::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("cmd-n", NewWindow, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &NewWindow, cx| {
            main_window(cx);
        });
        cx.on_action(|_: &SaveWorkspace, cx| watcher::get(cx).update(cx, |watcher, cx| watcher.save_now(cx)));
        cx.on_action(|_: &RestoreWorkspace, cx| {
            watcher::get(cx).update(cx, |watcher, cx| watcher.restore_latest(cx))
        });
        set_menus(cx);

        watcher::init(cx);
        let clicks = alerts::start();
        cx.spawn(async move |cx| {
            while let Ok(click) = clicks.recv().await {
                cx.update(|cx| {
                    if let Some(main) = main_window(cx) {
                        let _ = main.update(cx, |shell, window, cx| {
                            shell.open_tmux(&click.session, Some(&click.window_id), window, cx)
                        });
                    }
                });
            }
        })
        .detach();
        login::open_at_login_once();

        if let Some(main) = open_main_window(openings, cx)
            && go_to
        {
            let _ = main.update(cx, |shell, window, cx| shell.go_to_when_ready(window, cx));
        }
    });
}
