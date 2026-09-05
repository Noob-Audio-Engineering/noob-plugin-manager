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

mod install;
mod registry;
mod state;

use registry::Manifest;
use state::State;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("list", &[][..]),
    };

    let code = match cmd {
        "list" | "status" => cmd_list(),
        "install" => cmd_install(rest, false),
        "update" | "upgrade" => cmd_install(rest, true),
        "uninstall" | "remove" => cmd_uninstall(rest),
        "where" => cmd_where(),
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
    println!("  noob list                 what exists, what is installed, what is behind");
    println!("  noob install <id|all>     install or replace");
    println!("  noob update [id|all]      only what is behind (default: all)");
    println!("  noob uninstall <id>       remove exactly what an install added");
    println!("  noob where                the directories things go into");
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("noob-plugin-manager/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

/// What each plug-in's state is, said in the words a reader wants.
enum Status {
    Missing,
    Current,
    Behind(String),
    /// On disk, but this program did not put it there, so its build is unknown.
    Unknown,
}

fn status_of(m: &Manifest, st: &State) -> Status {
    match st.installed.get(&m.id) {
        Some(r) if r.commit == m.commit => Status::Current,
        Some(r) => Status::Behind(r.version.clone()),
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
            Status::Behind(v) => format!("{v} installed --- behind"),
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

fn cmd_install(rest: &[String], only_behind: bool) -> i32 {
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
        print!("{} {} ({}) ... ", m.id, m.version, m.short_commit());
        use std::io::Write;
        let _ = std::io::stdout().flush();

        match install::install(&ag, m) {
            Ok(record) => {
                println!("installed");
                st.installed.insert(m.id.clone(), record);
                done += 1;
            }
            Err(e) => {
                println!("failed");
                eprintln!("  {e}");
                failed += 1;
            }
        }
    }

    if let Err(e) = st.save() {
        eprintln!("note: the install record could not be written: {e}");
    }

    if skipped > 0 {
        println!("{skipped} already up to date.");
    }
    println!("{done} installed, {failed} failed.");
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
    for into in ["vst3", "clap"] {
        match state::install_dir(into) {
            Ok(p) => println!("{into:<5} {}", p.display()),
            Err(e) => println!("{into:<5} {e}"),
        }
    }
    0
}
