//! lazydisk desktop: the gpui frontend.

mod keys;
mod sunburst;
mod view;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use clap::Parser;
use gpui::{
    App, Bounds, Entity, KeyBinding, Menu, MenuItem, PathPromptOptions, QuitMode, TitlebarOptions,
    WindowBounds, WindowOptions, actions, prelude::*, px, size,
};
use lazydisk_core::config::{self, Config};
use lazydisk_core::keymap::Command;
use lazydisk_core::state::{Action, AppState};
use lazydisk_core::tree::SizeMode;

use crate::view::LazyDiskView;

/// Keyboard-driven disk space analyzer with a sunburst map (desktop).
#[derive(Parser, Debug)]
#[command(name = "lazydisk-desktop", version, about)]
struct Args {
    /// Folder or volume to scan (default: home folder).
    path: Option<PathBuf>,
    /// Show apparent (logical) sizes instead of allocated size on disk.
    #[arg(short = 'A', long)]
    apparent: bool,
    /// Number of sunburst rings (1-8).
    #[arg(long)]
    rings: Option<u8>,
    /// Print an example configuration file and exit.
    #[arg(long)]
    print_config: bool,
}

actions!(
    lazydisk,
    [
        Quit, OpenFolder, RescanAll, ShowHelp, Preview, Reveal, ToTrash, GoBack, GoForward, GoUp
    ]
);

fn main() {
    let args = Args::parse();
    if args.print_config {
        print!("{}", config::example());
        return;
    }
    let (mut cfg, _) = config::load().unwrap_or_else(|e| {
        eprintln!("warning: {e}; using defaults");
        (Config::default(), None)
    });
    if args.apparent {
        cfg.size_mode = SizeMode::Apparent;
    }
    if let Some(r) = args.rings {
        cfg.rings = r.clamp(1, 8);
    }
    let path = args
        .path
        .or_else(dirs_home)
        .unwrap_or_else(|| PathBuf::from("/"));
    let path = std::fs::canonicalize(&path).unwrap_or(path);

    gpui_platform::application()
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
            // gpui-ce 0.3.0 on macOS still holds its platform lock while this closure runs
            // (`did_finish_launching` keeps the guard alive across the callback), so any
            // platform call made here deadlocks. Defer the real setup to the first task
            // on the foreground executor, which runs once the callback has returned.
            cx.spawn(async move |cx| {
                cx.update(|cx| setup(cx, path, cfg));
            })
            .detach();
        });
}

fn setup(cx: &mut App, path: PathBuf, cfg: Config) {
    {
        {
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-o", OpenFolder, None),
                KeyBinding::new("cmd-shift-r", RescanAll, None),
                KeyBinding::new("cmd-[", GoBack, None),
                KeyBinding::new("cmd-]", GoForward, None),
                KeyBinding::new("cmd-up", GoUp, None),
                KeyBinding::new("cmd-backspace", ToTrash, None),
            ]);
            cx.set_menus([
                Menu::new("lazydisk").items([
                    MenuItem::action("Keyboard Shortcuts", ShowHelp),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ]),
                Menu::new("File").items([
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::action("Rescan Everything", RescanAll),
                    MenuItem::separator(),
                    MenuItem::action("Reveal in Finder", Reveal),
                    MenuItem::action("Quick Look", Preview),
                    MenuItem::separator(),
                    MenuItem::action("Move Collected to Trash", ToTrash),
                ]),
                Menu::new("Go").items([
                    MenuItem::action("Enclosing Folder", GoUp),
                    MenuItem::action("Back", GoBack),
                    MenuItem::action("Forward", GoForward),
                ]),
            ]);

            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let title = format!("lazydisk — {}", path.display());
            let state = AppState::new(&path, &cfg);
            let slot: Rc<RefCell<Option<Entity<LazyDiskView>>>> = Rc::new(RefCell::new(None));
            let slot2 = slot.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let entity = cx.new(|cx| {
                        let mut state = state;
                        state.start_scan();
                        LazyDiskView::new(state, window, cx)
                    });
                    *slot2.borrow_mut() = Some(entity.clone());
                    entity
                },
            )
            .expect("open window");

            // Menu actions dispatch into the view.
            let view = slot.borrow_mut().take();
            if let Some(view) = view {
                let v = view.clone();
                cx.on_action(move |_: &OpenFolder, cx| {
                    let rx = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Scan".into()),
                    });
                    let v = v.clone();
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(Some(paths))) = rx.await
                            && let Some(p) = paths.into_iter().next()
                        {
                            v.update(cx, |view, cx| view.apply(Action::ScanPath(p), cx));
                        }
                    })
                    .detach();
                });
                let v = view.clone();
                cx.on_action(move |_: &RescanAll, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::RescanAll), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &ShowHelp, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Help), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &Preview, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Preview), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &Reveal, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Reveal), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &ToTrash, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::DeleteTrash), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &GoBack, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Back), cx)
                    });
                });
                let v = view.clone();
                cx.on_action(move |_: &GoForward, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Forward), cx)
                    });
                });
                let v = view;
                cx.on_action(move |_: &GoUp, cx| {
                    v.update(cx, |view, cx| {
                        view.apply(Action::Command(Command::Parent), cx)
                    });
                });
            }
            cx.activate(true);
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
