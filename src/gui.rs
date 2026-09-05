//! The window.
//!
//! Every plug-in in this organisation puts its interface in a web view, so the
//! program that installs them does too --- the same `wry` at the same version.
//! It opens its own window rather than borrowing a host's, which is the one
//! way it differs from the plug-ins and the reason it uses `tao` directly
//! rather than the framework's webview crate.
//!
//! **The work does not run on the UI thread.** A download takes seconds and a
//! window that stops repainting during one looks broken, so a click posts a
//! message, a worker thread does the work, and the result comes back to be
//! drawn. The page disables itself while it waits, so there is no state in
//! which a second click starts the same install twice.

use std::sync::mpsc;

use serde::Serialize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use crate::install;
use crate::registry::{self, Display, Manifest};
use crate::settings::Settings;
use crate::state::{self, State};

/// What the page is told. One shape for every update, so a redraw never has to
/// merge two sources.
#[derive(Serialize)]
struct View {
    plugins: Vec<Row>,
    paths: Vec<(String, String)>,
    /// Something went wrong and the list may be incomplete.
    problem: Option<String>,
    /// Something worth saying that is not a failure.
    note: Option<String>,
    /// Whether the last failure is one administrator rights would fix. Only
    /// then is elevation offered --- a prompt that fails anyway teaches people
    /// to click through the next one.
    #[serde(rename = "canElevate")]
    can_elevate: bool,
    settings: SettingsView,
}

/// What the settings panel needs: the choices, plus the fact that decides
/// whether one of them is a preference or already forced.
#[derive(Serialize)]
struct SettingsView {
    prefer_user_dirs: bool,
    token: String,
    shared_writable: bool,
}

#[derive(Serialize)]
struct Row {
    id: String,
    version: String,
    commit: String,
    built: String,
    /// `current`, `behind`, `missing` or `unknown` --- the same four the
    /// command line prints, named the same way.
    state: &'static str,
    /// The version that is installed, when it is behind.
    installed: Option<String>,
    /// How the plug-in describes itself, in its own words.
    display: Option<Display>,
    /// A photograph of this build running, published beside it. `None` when
    /// the pipeline could not take one.
    banner: Option<String>,
}

/// What the page can ask for.
#[derive(serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Cmd {
    Refresh,
    Install {
        id: String,
    },
    InstallAll,
    Uninstall {
        id: String,
    },
    SaveSettings {
        prefer_user_dirs: bool,
        token: String,
    },
    Elevate,
}

/// Open the window. Returns when it closes.
pub fn run() -> i32 {
    let event_loop = EventLoopBuilder::<View>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = match WindowBuilder::new()
        .with_title("Noob Plugin Manager")
        .with_inner_size(tao::dpi::LogicalSize::new(760.0, 420.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(560.0, 260.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not open a window: {e}");
            return 1;
        }
    };

    // Commands go to a worker thread; results come back as user events.
    let (tx, rx) = mpsc::channel::<Cmd>();
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || worker(rx, proxy));
    }

    let built = WebViewBuilder::new()
        .with_html(include_str!("../ui/index.html"))
        .with_background_color((13, 16, 22, 255))
        .with_ipc_handler(move |req| match serde_json::from_str::<Cmd>(req.body()) {
            Ok(cmd) => {
                let _ = tx.send(cmd);
            }
            // A message this build does not understand means the page and the
            // program disagree about their own contract. Say so rather than
            // dropping it.
            Err(e) => eprintln!("the page sent something unreadable: {e}"),
        })
        .build(&window);

    let webview = match built {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not create the web view: {e}");
            return 1;
        }
    };

    event_loop.run(move |event, _, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control = ControlFlow::Exit,
            Event::UserEvent(view) => {
                let json = serde_json::to_string(&view).unwrap_or_else(|_| "null".into());
                // `applyState` is the page's only entry point, so a redraw has
                // exactly one path through.
                let _ = webview.evaluate_script(&format!("window.applyState({json})"));
            }
            _ => {}
        }
    });
}

/// Everything that touches the network or the disk, off the UI thread.
fn worker(rx: mpsc::Receiver<Cmd>, proxy: tao::event_loop::EventLoopProxy<View>) {
    let agent = crate::agent();
    while let Ok(cmd) = rx.recv() {
        let (found, problems) = registry::discover(&agent);
        let mut st = State::load();
        let mut note = None;

        match cmd {
            Cmd::Refresh => {}
            Cmd::Install { id } => {
                note = Some(install_these(&agent, &found, &mut st, |m| m.id == id));
            }
            Cmd::InstallAll => {
                note = Some(install_these(&agent, &found, &mut st, |_| true));
            }
            Cmd::SaveSettings {
                prefer_user_dirs,
                token,
            } => {
                let s = Settings {
                    prefer_user_dirs,
                    token: (!token.trim().is_empty()).then(|| token.trim().to_string()),
                };
                note = Some(match s.save() {
                    Ok(()) => "settings saved".to_string(),
                    Err(e) => format!("settings could not be saved: {e}"),
                });
            }
            // Elevation re-runs the whole install with administrator rights and
            // leaves this window alone --- the elevated copy writes the same
            // record, so refreshing afterwards shows what it did.
            Cmd::Elevate => {
                note = Some(
                    match crate::elevate::relaunch(&["install".into(), "all".into()]) {
                        Ok(()) => "asked for administrator; the elevated window will install,                                    then press Refresh here"
                            .to_string(),
                        Err(e) => e,
                    },
                );
            }
            Cmd::Uninstall { id } => {
                note = Some(match st.installed.get(&id).cloned() {
                    Some(rec) => match install::uninstall(&rec) {
                        Ok(n) => {
                            st.installed.remove(&id);
                            let _ = st.save();
                            format!("{id}: removed {n} item(s)")
                        }
                        Err(e) => format!("{id}: {}", first_line(&e)),
                    },
                    None => {
                        format!("{id} was not installed by this program, so it will not remove it")
                    }
                });
            }
        }

        let _ = proxy.send_event(view(&found, &problems, &st, note));
    }
}

fn install_these(
    agent: &ureq::Agent,
    found: &[Manifest],
    st: &mut State,
    want: impl Fn(&Manifest) -> bool,
) -> String {
    let mut done = 0;
    let mut failed: Vec<String> = Vec::new();
    for m in found.iter().filter(|m| want(m)) {
        match install::install(agent, m) {
            Ok(rec) => {
                st.installed.insert(m.id.clone(), rec);
                done += 1;
            }
            // The first line is the fault; the rest is advice the footer has
            // no room for and the command line already prints.
            Err(e) => failed.push(format!("{}: {}", m.id, first_line(&e))),
        }
    }
    if let Err(e) = st.save() {
        failed.push(format!("the install record could not be written: {e}"));
    }
    if failed.is_empty() {
        format!("{done} installed")
    } else {
        format!("{done} installed. {}", failed.join("  |  "))
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("failed").trim()
}

fn view(found: &[Manifest], problems: &[String], st: &State, note: Option<String>) -> View {
    let plugins = found
        .iter()
        .map(|m| {
            let (state, installed) = match st.installed.get(&m.id) {
                Some(r) if r.commit == m.commit => ("current", None),
                Some(r) => ("behind", Some(r.version.clone())),
                None if install::targets(m).iter().any(|(_, p)| p.exists()) => ("unknown", None),
                None => ("missing", None),
            };
            Row {
                id: m.id.clone(),
                version: m.version.clone(),
                commit: m.short_commit().to_string(),
                built: m.built_day().to_string(),
                state,
                installed,
                display: m.display.clone(),
                banner: m.banner.clone(),
            }
        })
        .collect();

    let paths = ["vst3", "clap"]
        .iter()
        .map(|k| {
            let v = state::install_dir(k).unwrap_or_else(std::path::PathBuf::from);
            ((*k).to_string(), v.display().to_string())
        })
        .collect();

    // The marker is put there by the installer when --- and only when --- it
    // has established that the *directory* refused, which is the one case
    // administrator rights change.
    let can_elevate = note.as_deref().is_some_and(|n| n.contains("ELEVATABLE"));
    let note = note.map(|n| n.replace("ELEVATABLE", "").trim().to_string());

    let s = Settings::load();
    let settings = SettingsView {
        prefer_user_dirs: s.prefer_user_dirs,
        token: s.token.clone().unwrap_or_default(),
        shared_writable: state::shared_writable(),
    };

    View {
        plugins,
        paths,
        problem: (!problems.is_empty()).then(|| problems.join("  |  ")),
        note,
        can_elevate,
        settings,
    }
}
