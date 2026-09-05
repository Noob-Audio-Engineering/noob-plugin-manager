# Noob Plugin Manager

`noob` installs and updates every Noob Audio Engineering plug-in, from the
builds their own repositories publish. Windows and macOS.

Running it with no arguments opens a window: a left-hand list of what is
published, a page for each plug-in saying what it is and what it does, and a
button to install it. Every plug-in in this organisation puts its interface
in a web view, and the program that installs them is not the exception.

The subcommands stay, for scripting:

```sh
noob list                 # what exists, what is installed, what is behind
noob install all          # install or replace every one
noob install noob-q       # or just one
noob update               # only what is behind
noob uninstall noob-q     # remove exactly what an install added
noob where                # the directories things go into
```

### The pictures are real

A plug-in's banner is a photograph of that build running, taken by its own
pipeline: the standalone serves exactly the page the plug-in embeds over
exactly the same bridge, so the picture is the plug-in rather than a drawing
of one, and it cannot go stale, because it is taken from the build it ships
beside. A build published before this existed simply has no picture.

Instruments and effects are listed apart, using the `kind` each plug-in
states in its own crate manifest. One that has not said what it is is filed
under neither: a synth listed among the effects is worse than a synth listed
under nothing.

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
                { "kind": "clap", "path": "noob-resonator.clap", "into": "clap" } ],
  "display": { "name": "Noob Resonator", "kind": "effect", "accent": "#4fd6c8",
               "tagline": "…", "features": ["…"] },
  "banner": "…/noob-resonator-banner.png" }
```

That file is the contract. This program reads it to decide whether it needs the
download at all, and to know where each part belongs.

`display` and `banner` are how a plug-in presents itself, and both come from
the plug-in: `display` out of `[package.metadata.noob]` in its own crate
manifest, `banner` from the photograph its pipeline takes of that build
running. Neither is written here. An installer holding a description of
somebody else's work would be a description kept where its author never looks,
and it would be wrong the first time they changed anything.

Both are optional. A build published before they existed, or a crate that has
not filled them in, simply shows less --- which is not the same as being
broken, and is not treated as though it were.

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
