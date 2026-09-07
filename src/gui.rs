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

use std::collections::BTreeMap;
use std::sync::mpsc;

use serde::Serialize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use crate::install;
use crate::registry::{self, Display, Manifest};
use crate::settings::Settings;
use crate::state::{self, Record, State};

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
        // Wide enough for the navigation beside two cards, and tall enough
        // that a plug-in's page shows its picture, its facts and the button
        // without scrolling.
        .with_inner_size(tao::dpi::LogicalSize::new(1120.0, 780.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(680.0, 420.0))
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

/// Print the state the window would be handed, and nothing else.
///
/// The page is drawn entirely from this one value, so a check that renders
/// *this* is checking what the window shows. `tools/page-check.mjs` can write
/// its own state instead, which is enough to work on the layout offline, but a
/// state written by hand can never disagree with the program --- and
/// disagreeing with the program is the only interesting thing a check of a
/// page can do.
pub fn dump() -> i32 {
    let agent = crate::agent();
    let (found, problems) = registry::discover(&agent);
    let st = State::load();
    let v = view(&found, &problems, &st, None);
    match serde_json::to_string_pretty(&v) {
        Ok(s) => {
            println!("{s}");
            0
        }
        Err(e) => {
            eprintln!("could not serialise the view: {e}");
            1
        }
    }
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
            // Elevation re-runs the whole install with administrator rights
            // and waits for it.
            //
            // **The elevated copy does not write the record; it prints it.**
            // It runs as root, and the record's location is resolved against
            // `$HOME` --- so it would either save into root's home, where this
            // window never looks, or save this user's file *as root*, after
            // which no unelevated install could ever write it again. Either
            // way the window would go on reporting these plug-ins as installed
            // by somebody else. So it hands the record back on stdout and this
            // process --- still the user --- saves it.
            Cmd::Elevate => {
                note = Some(
                    match crate::elevate::relaunch(&[
                        "install-elevated".into(),
                        "all".into(),
                        "--emit-record".into(),
                    ]) {
                        Ok(out) => adopt_elevated_record(&out, &mut st),
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

/// Take the record the elevated copy printed and save it under this account.
///
/// Anything unreadable is reported rather than swallowed: the install really
/// did happen, and a window that says nothing about it would send somebody to
/// do it again.
fn adopt_elevated_record(out: &str, st: &mut State) -> String {
    // `do shell script` returns the whole of stdout, which in this mode is the
    // record and nothing else --- everything human went to stderr. An empty
    // result means it installed nothing.
    let line = out.trim();
    if line.is_empty() {
        return "the elevated install reported nothing; press Refresh to see what is there"
            .to_string();
    }
    match serde_json::from_str::<BTreeMap<String, Record>>(line) {
        Ok(installed) => {
            let n = installed.len();
            st.installed.extend(installed);
            match st.save() {
                Ok(()) => format!("installed {n} with administrator"),
                Err(e) => format!(
                    "installed {n} with administrator, but the record could not be saved: {e}"
                ),
            }
        }
        Err(e) => format!(
            "the elevated install finished, but its record could not be read ({e}); press Refresh"
        ),
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
    // **Carried separately, because the note is shortened.** The marker the
    // installer sets is on the third line of the error and the footer keeps
    // only the first, so folding one into the other dropped it every time ---
    // and `can_elevate` reads the note. The offer of administrator was
    // therefore never made on the one failure it exists for: a plug-in folder
    // that refuses writes.
    let mut elevatable = false;
    for m in found.iter().filter(|m| want(m)) {
        match install::install(agent, m) {
            Ok(rec) => {
                st.installed.insert(m.id.clone(), rec);
                done += 1;
            }
            // The first line is the fault; the rest is advice the footer has
            // no room for and the command line already prints.
            Err(f) => {
                // Anything that landed before the failure is still recorded,
                // so the window's view and the plug-in folder agree and the
                // uninstall button can undo it.
                if let Some(partial) = f.partial {
                    st.installed.insert(m.id.clone(), partial);
                }
                elevatable |= f.error.contains(ELEVATABLE);
                failed.push(format!("{}: {}", m.id, first_line(&f.error)));
            }
        }
    }
    if let Err(e) = st.save() {
        failed.push(format!("the install record could not be written: {e}"));
    }
    note(done, &failed, elevatable)
}

/// The footer's line for an install, and the marker if administrator would
/// help.
///
/// Split out from `install_these` so it can be tested without a network: the
/// marker being dropped here is the whole of the bug, and it is not something
/// a reader spots by looking at either half.
fn note(done: usize, failed: &[String], elevatable: bool) -> String {
    let mut note = if failed.is_empty() {
        format!("{done} installed")
    } else {
        format!("{done} installed. {}", failed.join("  |  "))
    };
    // Re-attached to the whole note, which is what `view` reads it off and
    // then strips before anybody sees it.
    if elevatable {
        note.push(' ');
        note.push_str(ELEVATABLE);
    }
    note
}

/// The word the installer puts in an error to mean "administrator would fix
/// this", and the window turns into the offer. Named once so the two ends
/// cannot drift apart --- they already had.
pub const ELEVATABLE: &str = "ELEVATABLE";

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("failed").trim()
}

fn view(found: &[Manifest], problems: &[String], st: &State, note: Option<String>) -> View {
    let plugins = found
        .iter()
        .map(|m| {
            let (state, installed) = match st.installed.get(&m.id) {
                // Platform as well as commit --- see `Record::is_current` ---
                // and then the disk, because a record can describe an install
                // that was refused one of its three parts.
                Some(r) if r.is_current(&m.commit) && install::missing_parts(m).is_empty() => {
                    ("current", None)
                }
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

    // Every kind this machine hosts. On macOS that includes `au`, whose
    // `Components` directory the installer has always written to and the
    // window never showed.
    let paths = state::kinds_here()
        .into_iter()
        .map(|k| {
            let v = state::install_dir(k).unwrap_or_else(std::path::PathBuf::from);
            (k.to_string(), v.display().to_string())
        })
        .collect();

    // The marker is put there by the installer when --- and only when --- it
    // has established that the *directory* refused, which is the one case
    // administrator rights change.
    let can_elevate = note.as_deref().is_some_and(|n| n.contains(ELEVATABLE));
    let note = note.map(|n| n.replace(ELEVATABLE, "").trim().to_string());

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The installer's message for a folder that refuses writes, as
    /// `install::locked` builds it: the fault first, the advice next, and the
    /// marker last.
    const REFUSED: &str = "/Library/Audio/Plug-Ins/Components/x.component could not be written: \
         Permission denied (os error 13)\n  The folder itself refuses writes. \
         Administrator rights would fix this.\n  ELEVATABLE";

    /// **The marker is not on the first line, and the footer keeps only the
    /// first line.**
    ///
    /// This is the whole of the bug: the note was built from `first_line`, so
    /// the marker was thrown away before `view` looked for it, `can_elevate`
    /// was false, and the offer of administrator was never made --- on exactly
    /// the failure it exists for. It is asserted here rather than described,
    /// because both halves read plausibly on their own.
    #[test]
    fn the_marker_cannot_survive_shortening_so_it_is_carried_separately() {
        assert!(
            REFUSED.contains(ELEVATABLE),
            "the installer must mark a refused directory"
        );
        assert!(
            !first_line(REFUSED).contains(ELEVATABLE),
            "if the marker were on the first line this test would prove nothing"
        );

        // The footer's real line, built the way `install_these` builds it.
        let line = format!("x: {}", first_line(REFUSED));
        let note = note(0, &[line], true);
        assert!(
            note.contains(ELEVATABLE),
            "the note lost the marker again, so no administrator is offered: {note}"
        );
    }

    /// And `view` reads it off the note and then strips it, so the marker is
    /// never shown to anybody.
    #[test]
    fn the_marker_is_read_and_then_removed() {
        let note = note(0, &[format!("x: {}", first_line(REFUSED))], true);
        let can_elevate = note.contains(ELEVATABLE);
        let shown = note.replace(ELEVATABLE, "").trim().to_string();
        assert!(can_elevate, "the offer was not made");
        assert!(
            !shown.contains(ELEVATABLE),
            "the marker was shown to the user: {shown}"
        );
        assert!(
            shown.contains("could not be written"),
            "stripping the marker took the message with it: {shown}"
        );
    }

    /// A note from an install that succeeded carries no marker, so no offer of
    /// administrator is made when nothing refused.
    #[test]
    fn a_clean_install_offers_no_administrator() {
        let note = note(6, &[], false);
        assert_eq!(note, "6 installed");
        assert!(!note.contains(ELEVATABLE));
    }

    /// A failure administrator cannot fix --- a plug-in held open by a running
    /// host --- must not raise the offer either. Elevation does nothing for a
    /// loaded library, and a prompt that fails anyway teaches people to click
    /// through the next one.
    #[test]
    fn a_locked_file_offers_no_administrator() {
        let note = note(0, &["x: in use".to_string()], false);
        assert!(
            !note.contains(ELEVATABLE),
            "administrator was offered for something it cannot fix: {note}"
        );
        assert!(note.contains("in use"));
    }
}
