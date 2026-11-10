<p align="center">
  <img src="assets/cassette-150.png" width="88" alt="Cassette app icon">
</p>
<h1 align="center">Cassette</h1>
<p align="center">YouTube Music and your local collection in one native desktop player.</p>

## Download

Get the latest stable release for **Windows x64** (portable EXE), **macOS Apple Silicon** (DMG), or **Linux x64** (DEB built on Ubuntu 24.04). [Download Cassette](https://github.com/ctrlcat0x/Cassette/releases/latest)

On macOS, drag Cassette into Applications. The build is ad-hoc signed. On Ubuntu, run `sudo apt install ./cassette-linux-x64.deb`. Windows sign-in requires Microsoft Edge WebView2.

## Features

- **Discover music.** Search YouTube Music for songs, albums, artists and playlists. Browse charts, moods and new releases, or start a song radio.
- **Browse your library.** Import files and folders, browse album artwork, play albums in disc order and rescan folders to refresh tags and covers.
- **Organize your collection.** Like songs, build playlists and smart playlists, group them in folders, and import or export playlists. Connect YouTube Music to access and sync your account library.
- **Listen offline.** Pin songs or collections for later, manage your cache, and use Offline mode to stay with downloaded tracks and local files.
- **Control playback.** Edit the queue, shuffle or repeat, set a sleep timer, and tune crossfade, normalization, equalizer, audio output and quality. Use system media controls and the Windows taskbar controls.
- **Follow the lyrics.** View synced lyrics and available word timing, translations and romanization. Adjust timing or import a corrected lyrics file.
- **Make Cassette yours.** Choose themes, accent colors, interface scale and language. Move playback controls between sidebar and bottom bar, switch to fullscreen, or enable Reduce motion.
- **See your listening story.** Review recent tracks and top songs, artists and albums. Use a private session to pause listening history and automatic caching.

## Shortcuts

| Key | Action |
| --- | --- |
| Ctrl/⌘ + K | Focus search |
| Ctrl/⌘ + O | Import local files |
| Ctrl/⌘ + , | Open Settings |
| Ctrl/⌘ + B | Toggle sidebar |
| Space | Play or pause |
| F11 | Toggle fullscreen player |

## Build

Install stable Rust, Python 3.12 and CMake. Windows needs Visual Studio C++ Build Tools and the Windows SDK. macOS needs Xcode Command Line Tools. Linux dependencies are listed in the [build workflow](.github/workflows/build.yml).

```sh
python scripts/bundle-runtime.py
cargo build --release --locked --features bundled-runtime --bin cassette
```

Find the executable in `target/release`. Release builds bundle yt-dlp and QuickJS-NG. Run `cassette --verify-runtime` to check the bundled tools or `cassette --verify-audio /path/to/track.wav` to check native audio output. For development, run `cargo run --locked` with yt-dlp and a supported JavaScript runtime on your PATH.

Cassette is built with Rust, GPUI Kit, CPAL and Symphonia. The Cargo package name is `raag`. [MIT License](LICENSE)
