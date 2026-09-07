//! `noob` --- installs and updates every Noob Audio Engineering plug-in.
//!
//! ```text
//! noob list                 what exists, what is installed, what is behind
//! noob install <id|all>     install or replace
//! noob update [id|all]      only what is behind
//! noob uninstall <id>       remove exactly what an install added
//! noob where                the directories things go into
//! ```
//!
//! It has no list of plug-ins in it. It asks the organisation what it
//! publishes, and treats a repository that publishes a build manifest as a
//! plug-in --- so a new one appears here the moment its own pipeline first
//! runs, without this program being edited or re-released.

mod elevate;
mod gui;
mod install;
mod registry;
mod settings;
mod state;

use registry::Manifest;
use state::State;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // No arguments opens the window. Every plug-in in this organisation puts
    // its interface in a web view, and the program that installs them should
    // not be the exception --- the subcommands stay for scripting.
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("gui", &[][..]),
    };

    let code = match cmd {
        "gui" | "window" => gui::run(),
        "list" | "status" => cmd_list(),
        "install" => cmd_install(rest, false),
        "update" | "upgrade" => cmd_install(rest, true),
        // Not in the help: this is how the elevated copy talks back to the
        // one that raised it. See `cmd_install`'s `Emit`.
        "install-elevated" => cmd_install(rest, false),
        "uninstall" | "remove" => cmd_uninstall(rest),
        "where" => cmd_where(),
        // Used by `tools/page-check.mjs`, so the interface can be drawn
        // against the state this program actually sends.
        "view" => gui::dump(),
        "-h" | "--help" | "help" => {
            help();
            0
        }
        other => {
            eprintln!("unknown command `{other}`\n");
            help();
            2
        }
    };
    std::process::exit(code);
}

fn help() {
    println!("{}", env!("CARGO_PKG_DESCRIPTION"));
    println!();
    println!("  noob                      open the window (this is the usual way)");
    println!("  noob list                 what exists, what is installed, what is behind");
    println!("  noob install <id|all>     install or replace");
    println!("  noob update [id|all]      only what is behind (default: all)");
    println!("  noob uninstall <id>       remove exactly what an install added");
    println!("  noob where                the directories things go into");
    println!("  noob view                 print the state the window is drawn from, as JSON");
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("noob-plugin-manager/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

/// What each plug-in's state is, said in the words a reader wants.
enum Status {
    Missing,
    Current,
    /// The installed version, and why it is not the current one --- which is
    /// usually "behind", is sometimes "this is another platform's build", and
    /// is sometimes a part that never made it onto disk.
    Behind(String, String),
    /// On disk, but this program did not put it there, so its build is unknown.
    Unknown,
}

fn status_of(m: &Manifest, st: &State) -> Status {
    match st.installed.get(&m.id) {
        // The commit *and* the platform. The commit alone called a wrongly
        // installed Windows bundle up to date, because both platforms build
        // the same commit --- so the corrected installer skipped exactly the
        // machines that needed it.
        Some(r) if r.is_current(&m.commit) => {
            // The record says the right commit for the right platform. Whether
            // every part of it actually reached the disk is a separate
            // question, and only the disk can answer it.
            match install::missing_parts(m).as_slice() {
                [] => Status::Current,
                missing => Status::Behind(
                    r.version.clone(),
                    format!("missing its {}", missing.join(" and ")),
                ),
            }
        }
        Some(r) => Status::Behind(r.version.clone(), r.why_not_current().to_string()),
        None => {
            let any = install::targets(m).iter().any(|(_, p)| p.exists());
            if any {
                Status::Unknown
            } else {
                Status::Missing
            }
        }
    }
}

fn cmd_list() -> i32 {
    let (found, problems) = registry::discover(&agent());
    let st = State::load();

    if found.is_empty() {
        // "No plug-ins found" with the reason underneath reads as a claim
        // about the plug-ins. When the reason is that nothing could be asked,
        // say that instead --- it is a different statement.
        if problems.is_empty() {
            eprintln!("No plug-ins found.");
        } else {
            eprintln!("Could not find out what is published:");
            for p in &problems {
                eprintln!("  {p}");
            }
        }
        return 1;
    }

    println!(
        "{:<24} {:<9} {:<8} {:<11} here",
        "plug-in", "version", "commit", "built"
    );
    for m in &found {
        let here = match status_of(m, &st) {
            Status::Current => "up to date".to_string(),
            Status::Behind(v, why) => format!("{v} installed --- {why}"),
            Status::Missing => "not installed".to_string(),
            Status::Unknown => "installed, build unknown".to_string(),
        };
        // The build date answers "how old is this?", which the version cannot:
        // these move with every commit and the version rarely changes.
        println!(
            "{:<24} {:<9} {:<8} {:<11} {}",
            m.id,
            m.version,
            m.short_commit(),
            m.built_day(),
            here
        );
    }

    for p in &problems {
        eprintln!("\nnote: {p}");
    }
    0
}

/// **How an elevated install gets its result back to the user.**
///
/// `osascript ... with administrator privileges` starts a *second process* as
/// root; there is no way to raise the rights of one already running. That
/// process then computes where the install record lives, and `directories`
/// resolves that against `$HOME` --- so the elevated copy either writes the
/// record into root's home, where the user's manager never looks, or writes
/// the user's file *as root*, after which every later unelevated save is
/// refused. The window said "the elevated copy writes the same record"; it
/// could not, whichever way `$HOME` went.
///
/// So the elevated copy writes no record at all. It prints what it installed
/// as JSON on stdout, which `do shell script` hands back as its result, and
/// the process that asked --- still running as the user --- merges it in and
/// saves. Everything human goes to stderr so the JSON is alone on stdout.
struct Emit(bool);

impl Emit {
    /// Progress, to wherever it belongs: stderr when stdout is carrying the
    /// record, stdout otherwise.
    fn say(&self, line: &str) {
        if self.0 {
            eprint!("{line}");
            let _ = std::io::Write::flush(&mut std::io::stderr());
        } else {
            print!("{line}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }
}

fn cmd_install(rest: &[String], only_behind: bool) -> i32 {
    // The flag is stripped before the plug-in name is read, so
    // `install-elevated all --emit-record` and `install all` parse alike.
    let emit = Emit(rest.iter().any(|a| a == "--emit-record"));
    let rest: Vec<String> = rest
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    let want = rest.first().map(String::as_str).unwrap_or("all");
    let ag = agent();
    let (found, problems) = registry::discover(&ag);
    for p in &problems {
        eprintln!("note: {p}");
    }
    if found.is_empty() {
        eprintln!("No plug-ins found.");
        return 1;
    }

    let mut st = State::load();
    let chosen: Vec<&Manifest> = if want == "all" {
        found.iter().collect()
    } else {
        match found.iter().find(|m| m.id == want) {
            Some(m) => vec![m],
            None => {
                eprintln!("No plug-in called `{want}`. Try `noob list`.");
                return 2;
            }
        }
    };

    let mut done = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for m in chosen {
        if only_behind && matches!(status_of(m, &st), Status::Current) {
            skipped += 1;
            continue;
        }
        emit.say(&format!(
            "{} {} ({}) ... ",
            m.id,
            m.version,
            m.short_commit()
        ));

        match install::install(&ag, m) {
            Ok(record) => {
                emit.say("installed\n");
                st.installed.insert(m.id.clone(), record);
                done += 1;
            }
            Err(f) => {
                emit.say("failed\n");
                eprintln!("  {}", f.error);
                // Whatever landed before the failure is recorded, or it is a
                // bundle in the plug-in folder that this program put there and
                // then denied all knowledge of --- and `uninstall` will not
                // remove what it has no record of.
                if let Some(partial) = f.partial {
                    for p in &partial.paths {
                        eprintln!("  left in place: {}", p.display());
                    }
                    eprintln!("  Recorded, so `noob uninstall {}` will remove them.", m.id);
                    st.installed.insert(m.id.clone(), partial);
                }
                failed += 1;
            }
        }
    }

    if emit.0 {
        // Root must not write the record --- see `Emit`. It is handed back on
        // stdout instead, and the process that asked for administrator saves
        // it under its own account.
        match serde_json::to_string(&st.installed) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("note: the install record could not be encoded: {e}"),
        }
    } else if let Err(e) = st.save() {
        eprintln!("note: the install record could not be written: {e}");
    }

    if skipped > 0 {
        emit.say(&format!("{skipped} already up to date.\n"));
    }
    emit.say(&format!("{done} installed, {failed} failed.\n"));
    // A partial success is a failure to whatever is scripting this.
    if failed > 0 { 1 } else { 0 }
}

fn cmd_uninstall(rest: &[String]) -> i32 {
    let Some(id) = rest.first() else {
        eprintln!("Which one? `noob uninstall <id>`, or `noob list` to see them.");
        return 2;
    };
    let mut st = State::load();
    let Some(record) = st.installed.get(id).cloned() else {
        eprintln!(
            "There is no record of installing `{id}`.\n  \
             If it is in the plug-in folder, this program did not put it there, \
             and it will not remove something it cannot account for."
        );
        return 1;
    };
    match install::uninstall(&record) {
        Ok(n) => {
            st.installed.remove(id);
            if let Err(e) = st.save() {
                eprintln!("note: the install record could not be written: {e}");
            }
            println!("{id}: removed {n} item(s).");
            0
        }
        Err(e) => {
            eprintln!("{id}: {e}");
            1
        }
    }
}

fn cmd_where() -> i32 {
    // Every kind this machine hosts, so macOS names `Components` too. A
    // `where` that omits the directory an Audio Unit is installed into is a
    // report of where things go that leaves one of them out.
    for into in state::kinds_here() {
        match state::install_dir(into) {
            Ok(p) => println!("{into:<5} {}", p.display()),
            Err(e) => println!("{into:<5} {e}"),
        }
    }
    0
}
