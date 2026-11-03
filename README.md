# TeMS Player

**TeMS Player** is a Rust audio player with a native desktop interface built with **eframe/egui** and an interactive **terminal (CLI) mode**. The GUI is the default when the application is launched without media in a graphical session; command-line media arguments start terminal playback. The GUI includes playlist editing, drag-and-drop, and an adjustable crossfade.

> **Version and CLI note:** This README is drafted for the requested 0.4.0 feature set. In the repository revision reviewed for this draft, `Cargo.toml` and the Clap command metadata still report `0.2.0`; update both before publishing a 0.4.0 release. That revision does **not** define a `--cli` option. See [Choosing an interface](#choosing-an-interface) for the actual invocation behavior.

## Features

- **Desktop GUI:** Native eframe/egui window with playback controls, track information, playlist search, and a position slider.
- **Terminal player:** Interactive CLI with playback, seeking, volume, shuffle, repeat, track lookup, and file information controls.
- **Audio decoding:** Symphonia (enabled with its `all` feature) with explicit Ogg/Opus support. The player recognizes files with these extensions: `.mp3`, `.flac`, `.aac`, `.m4a`, `.opus`, `.ogg`, `.wav`, and `.alac`. Playback still depends on the file being valid and containing a supported audio stream.
- **Playlist input:** Individual audio files, recursively scanned directories, and M3U playlists. The GUI can also open M3U8 playlists.
- **Playlist editing in the GUI:** Replace, append, or prepend files, folders, or playlists; select and remove tracks; move selected tracks after the current track; reorder tracks; clear the list; and save a playlist as M3U.
- **Drag-and-drop:** Drop supported audio files, folders, or M3U/M3U8 playlists onto the GUI to append their tracks.
- **Crossfade:** Adjustable GUI crossfade from 0 to 30 seconds. Set it to 0 to disable crossfade.
- **Playback options:** Play/pause, stop, next/previous track, shuffle, repeat (Off / All / One), seeking, and software volume control.

## Choosing an interface

The application uses one executable for the GUI and CLI; there are no separate Cargo features or binaries for the two modes.

- With **no media arguments**, it opens the GUI when a graphical session is detected. On Linux, the program checks `DISPLAY` and `WAYLAND_DISPLAY`; on macOS and Windows it treats a GUI as available.
- Pass **`--gui`** to request the GUI explicitly (you may also provide initial media inputs).
- Provide files, a directory, or a playlist without `--gui` to start the terminal player.
- There is **no `--cli` flag** in the reviewed source. To launch CLI mode, provide media input, for example `tems-player ./song.flac` or `tems-player --directory ./Music`.
- Without media arguments and without a detected graphical session, the program follows the CLI path; it cannot start playback until tracks are supplied.

## Build and installation

### Requirements

- A supported Rust toolchain and Cargo (the project uses Rust edition 2021).
- A working audio output device and the native audio, windowing, and dialog components required by your operating system and Rust target.

For source builds, platform development prerequisites typically include:

- **Linux:** a C toolchain and `pkg-config`; ALSA development headers/libraries for `cpal`; and the X11 and/or Wayland development packages needed by the enabled eframe backends. Package names vary by distribution. Ensure a graphical session is available to run the GUI.
- **macOS:** Xcode Command Line Tools (or Xcode) for the compiler and Apple SDK. The repository's `build.rs` links Apple system frameworks.
- **Windows:** the Rust MSVC toolchain and its corresponding Visual Studio C++ Build Tools/Windows SDK components.

The exact native prerequisites can differ between distributions and toolchain setups. The GUI dependencies are regular Cargo dependencies, not optional features, so a normal build compiles the GUI as well as the CLI.

### Build from source

```sh
git clone https://github.com/philtems/tems-player.git
cd tems-player
cargo build --release
```

The release executable is written under `target/release/` (`tems-player` on Unix-like systems; `tems-player.exe` on Windows).

To run directly from the checkout:

```sh
cargo run --release
cargo run --release -- --gui
cargo run --release -- ./song.flac
```

The repository also contains specialized `scripts/build-macos.sh` and `scripts/build-musl.sh` scripts for particular cross/portable build setups. These are not the standard build path and have additional toolchain and SDK/container prerequisites; consult each script before using it.

## Command-line usage

```text
tems-player [OPTIONS] [FILES]...
```

`FILES` may contain audio-file paths or directory paths. Directories are scanned recursively. The available source-defined options are:

| Option | Description |
| --- | --- |
| `-d`, `--directory <DIRECTORY>` | Recursively add audio files found in a directory. May be supplied more than once. |
| `-m`, `--m3u <M3U>` | Load tracks from an M3U playlist. May be supplied more than once. |
| `--gui` | Open the graphical interface instead of starting the terminal player. |
| `-h`, `--help` | Show command-line help (provided by Clap). |
| `-V`, `--version` | Show the version reported by the package/command metadata. |

Examples:

```sh
# Play one or more files in the terminal player
tems-player ./song.flac ./another-track.mp3

# Recursively scan a directory
tems-player --directory ./Music

# Load an M3U playlist
tems-player --m3u ./playlist.m3u

# Combine inputs
tems-player -d ./Albums -m ./favorites.m3u ./intro.wav

# Launch the GUI explicitly
tems-player --gui
```

M3U entries are read as paths and checked for existence; relative entries are not resolved relative to the playlist file, and extended M3U metadata is not interpreted as track metadata.

## Keyboard shortcuts (CLI)

These keys apply to the interactive terminal player, **not** to the GUI. The GUI is controlled through its buttons, menus, sliders, playlist selection, and context menu; the reviewed source does not define matching global keyboard shortcuts for it.

| Key | Action |
| --- | --- |
| `Space` | Play / pause |
| `n` or `↓` | Next track |
| `p` or `↑` | Previous track |
| `←` / `→` | Seek backward / forward by 5 seconds |
| `+` or `=` / `-` | Increase / decrease volume by 0.1 |
| `s` | Toggle shuffle |
| `r` | Cycle repeat: Off → All → One |
| `g` | Go to a track number |
| `/` | Search the playlist |
| `i` | Show information about the current file |
| `l` | List playlist tracks |
| `h` | Show help |
| `q` | Quit |

## GUI overview

Use the **File** menu to open or add files, folders, and playlists, choosing whether to replace the current list or prepend/append tracks. The **Playlist** menu saves the current list as an M3U file. Drag tracks to reorder them; use selection (including modifier-assisted multi-selection), the context menu, or the available playlist actions to play, remove, or move tracks. Search filters the displayed playlist by filename. Playback controls include a seek bar and ±5-second buttons, volume, crossfade, shuffle, and repeat.

