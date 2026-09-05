# Noob Plugin Manager

`noob` installs and updates every Noob Audio Engineering plug-in, from the
builds their own repositories publish. Windows and macOS.

```sh
noob list                 # what exists, what is installed, what is behind
noob install all          # install or replace every one
noob install noob-q       # or just one
noob update               # only what is behind
noob uninstall noob-q     # remove exactly what an install added
noob where                # the directories things go into
```

## It has no list of plug-ins in it

The obvious design is a registry file naming the plug-ins. It would have been
wrong the day a sixth shipped — added by somebody with no reason to think an
installer in another repository needed editing.

So this asks the organisation what it publishes, and treats **publishing a
build manifest** as the thing that makes a repository a plug-in. A new one
appears here the moment its own pipeline first runs, without this program
being changed or re-released; a repository that is not a plug-in never appears,
because it has no manifest to find.

## What it installs, and where

Each plug-in's pipeline builds a bundle for every commit on `main` and
publishes it to a rolling `latest` release, with a manifest beside it:

```json
{ "schema": 1, "id": "noob-resonator", "version": "0.1.0",
  "commit": "…", "built": "…", "platform": "windows-x86_64",
  "sha256": "…", "url": "…",
  "installs": [ { "kind": "vst3", "path": "noob-resonator.vst3", "into": "vst3" },
                { "kind": "clap", "path": "noob-resonator.clap", "into": "clap" } ] }
```

That file is the contract. This program reads it to decide whether it needs the
download at all, and to know where each part belongs.

| | VST3 | CLAP |
|---|---|---|
| Windows | `%CommonProgramFiles%\VST3` | `%CommonProgramFiles%\CLAP` |
| macOS | `~/Library/Audio/Plug-Ins/VST3` | `~/Library/Audio/Plug-Ins/CLAP` |

macOS uses **your** plug-in folder rather than `/Library`, deliberately: every
host scans both, it needs no administrator, and an installer that asks for a
password to put a free plug-in on your own machine is asking for more than it
needs.

## What it refuses to do

**It will not install a download that does not match its manifest's checksum.**
Nothing is written to disk at all in that case. A half-correct plug-in is worse
than none: it loads, it sounds wrong, and nothing says why.

**It will not remove something it did not install.** If a plug-in is in the
folder with no record of this program putting it there, `uninstall` says so and
stops rather than deleting a file it cannot account for. `list` reports that
state as *installed, build unknown* rather than as *not installed*, because an
installer that says "not installed" about something sitting in the plug-in
folder is wrong in the direction that makes people install it twice.

**It will not pretend a locked file is a mysterious failure.** A plug-in loaded
in a host is held open, so an install can fail partway through. Everything is
staged beside the target and moved in only once all of it has arrived, and a
lock is reported as what it is: close the project, or the host, and run it
again.

## Building it

```sh
cargo build --release      # target/release/noob
```

No async runtime: this makes a handful of requests and exits, so one would be a
dependency with nothing to do.
