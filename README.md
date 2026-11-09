<p align="center">
  <img src="assets/cassette-150.png" width="88" alt="Cassette app icon">
</p>
<h1 align="center">Cassette</h1>
<p align="center">YouTube Music and your local collection in one native desktop player.</p>
<p align="center">
  <a href="https://github.com/ctrlcat0x/Cassette/releases/tag/v0.9.0"><strong>Get v0.9</strong></a>
  · <a href="https://github.com/ctrlcat0x/Cassette/releases">All releases</a>
  · <a href="https://github.com/ctrlcat0x/Cassette/issues">Report an issue</a>
</p>

## Download

| Platform | Package | Install |
| --- | --- | --- |
| Windows x64 | [Portable EXE](https://github.com/ctrlcat0x/Cassette/releases/download/v0.9.0/cassette-windows-x64.exe) | Run the downloaded executable. |
| macOS Apple Silicon | [DMG](https://github.com/ctrlcat0x/Cassette/releases/download/v0.9.0/cassette-macos-arm64.dmg) | Open the disk image and drag Cassette into Applications. The build is ad-hoc signed. |
| Linux x64 | [DEB](https://github.com/ctrlcat0x/Cassette/releases/download/v0.9.0/cassette-linux-x64.deb) | Run `sudo apt install ./cassette-linux-x64.deb` on Ubuntu 24.04. |

Cassette bundles yt-dlp and QuickJS-NG. The first launch extracts them into its private data directory. Browsing works as a guest. Connect a YouTube Music account from the profile menu to sync your library. Google sign-in on Windows uses the [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).

## Listen your way

- **Discover music.** Search songs, albums, artists and playlists. Explore charts, moods, new releases and song radio.
- **Build a collection.** Import local files and folders, save favourites, organize playlists, and keep downloads for offline listening.
- **Make playback yours.** Manage the queue, follow synced lyrics, adjust crossfade and equalizer, or open the fullscreen player.
- **Tune the interface.** Choose a theme, accent, language and display scale. Enable **Reduce motion** in Appearance to limit motion in Cassette independently of the system setting.

| Shortcut | Action |
| --- | --- |
| Ctrl/⌘ + K | Focus search |
| Ctrl/⌘ + O | Import local files |
| Ctrl/⌘ + , | Open Settings |
| Ctrl/⌘ + B | Toggle sidebar |
| Space | Play or pause |
| F11 | Open or close fullscreen playback |

## What changed in v0.9

- Choose **Local files** in **Your library**, then **Add folder…** to browse your music by album cover. Open a cover to see its tracks or play the album.
- **Songs** shows the full local collection. **Rescan folders** refreshes artwork and track tags for previously added folders.

## What changed in v0.8

- Scrolling stays with the open popup. Artwork blends smoothly as it loads, and status notices fade in briefly. Reduce motion keeps loading indicators still.
- Cancelling a stream stops its decoder, and volume changes respect the crossfade. Offline mode uses local files and cached tracks.
- Private sessions skip automatic caching. Local scans preserve edits made while scanning, and exported playlists omit account-specific state.
- Library saves finish independently of the UI event queue.

## What changed in v0.7.1

- Home shelves keep visible cards mounted during rapid paging and reversals.
- The playback clock recovers when buffering interrupts a seek. Track labels and active lyrics update immediately.
- Fullscreen tint changes and the optional blurred backdrop fade smoothly. Page navigation and artwork hover respond immediately.

## What changed in v0.7.0

- Smaller downloads with QuickJS-NG and a newer yt-dlp. Windows uses the upstream directory build of yt-dlp to avoid extracting Python on each request.
- JioSaavn matching checks title, artist, duration and recording metadata. Playback can start from the first media range, with YouTube available as fallback.
- Less background polling, cached playback controls and finite transitions for pages, panes and artwork.
- Refreshed discovery, collection, settings and player surfaces, with direct access to local files and downloads. Windows adds taskbar playback controls.

## Build from source

Install [stable Rust](https://rustup.rs/), Python 3.12 and CMake. Windows needs Visual Studio C++ Build Tools and the Windows SDK. macOS needs Xcode Command Line Tools. See the [build workflow](.github/workflows/build.yml) for Linux packages.

```sh
git clone https://github.com/ctrlcat0x/Cassette.git
cd Cassette
python scripts/bundle-runtime.py
cargo build --release --locked --features bundled-runtime --bin cassette
```

Run the binary under `target/release`. Use `cassette --verify-runtime` to check the embedded tools, or `cassette --verify-audio /path/to/track.wav` to exercise native audio output. For a development build without embedded tools, use `cargo run --locked` and make yt-dlp and a supported JavaScript runtime available. Settings also accepts a custom yt-dlp executable.

Cassette uses Rust, GPUI Kit, CPAL and Symphonia. The Cargo package retains the internal name `raag`.

## License

Cassette is available under the [MIT License](LICENSE). Dependencies, icons, fonts and music artwork retain their respective licenses.
