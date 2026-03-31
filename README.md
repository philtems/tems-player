# TeMS Player

**TeMS Player** is a lightweight and fast command-line (CLI) audio player written in Rust. It provides an interactive terminal interface to manage playlists, support a wide range of audio formats, and precisely control playback.

## 🚀 Features

*   **Multi-format Support:** Native playback of MP3, FLAC, AAC, M4A, Opus, OGG, WAV, and ALAC powered by the `symphonia` decoding engine.
*   **Flexible Playlist Management:**
    *   Play individual files, entire directories (recursive), or `.m3u` playlist files.
    *   **Shuffle** and **Repeat** modes (Off, All, One).
    *   Quick search within the playlist and navigation by track number.
*   **Interactive Controls:**
    *   Play/Pause, Next/Previous track.
    *   Seek forward/backward in 5-second steps.
    *   Software volume adjustment.
    *   Display of technical metadata (bitrate, sample rate, duration).
*   **Modern Terminal Interface:** Uses progress bars, emojis, and colors for a pleasant user experience without leaving the shell.
*   **Portability:**
    *   Pure Rust architecture with minimal dependencies.
    *   Supports static compilation (`musl`) for standalone Linux binaries.
    *   Cross-platform audio abstraction via `cpal` (ALSA, CoreAudio, WASAPI).

## ⌨️ Keyboard Shortcuts

| Key | Action |
| :--- | :--- |
| `Space` | Play / Pause |
| `n` / `↓` | Next track |
| `p` / `↑` | Previous track |
| `←` / `→` | Seek -5s / +5s |
| `+` / `-` | Volume Up / Down |
| `s` | Toggle **Shuffle** mode |
| `r` | Cycle **Repeat** mode (Off → All → One) |
| `g` | Go to track (by number) |
| `/` | Search track (by name) |
| `i` | Show detailed file info |
| `l` | Show playlist |
| `h` | Help |
| `q` | Quit |

## 🛠️ Installation & Build

# Standard build (Release)
cargo build --release
