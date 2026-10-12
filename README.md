# Cassette

<img src="assets/cassette-150.png" width="80" alt="Cassette app icon">

A native desktop music player built with Rust and GPUI. The source project is named Raag.

Browse YouTube Music, play your local files, and keep your favourite music available offline.

## Features

- YouTube Music search, discovery, albums, artists, playlists and account library sync.
- Optional JioSaavn audio matching, with YouTube Music fallback.
- Local music, likes, playlists, listening history and pinned downloads.
- Synced lyrics from LRCLIB, BiniLyrics, lrc.red and LyricsPlus, with additional fallbacks.
- Queue controls, equalizer, crossfade, themes, fullscreen player and system media controls.
- Protected account sessions and bounded artwork, metadata and audio caches.

## Run on Windows

Install stable [Rust](https://rustup.rs/), Visual Studio C++ Build Tools and the Windows SDK. Google sign-in also needs the Microsoft Edge WebView2 Runtime.

```powershell
./scripts/install-playback-tools.ps1
cargo run --release --locked
```

The installer verifies upstream checksums and installs mpv, yt-dlp and Deno in `.runtime`. Keep those tools current when YouTube changes playback requirements.

For a Windows EXE with playback tools inside, run `cargo build --release --locked --features bundled-runtime`, or use `scripts/package-windows.ps1 -Format exe`. Distribute `cassette.exe`; mpv, Deno and the fast unpacked yt-dlp runtime are compressed inside it. First launch extracts and verifies them in protected local application storage. Later launches reuse that copy. Google sign-in still needs WebView2. Update the installed tools and rebuild to ship newer playback tools.

You can browse without signing in. Use the avatar menu to connect YouTube Music. Lyrics and JioSaavn need no separate account.

Cassette is in development. Provider availability can change. Windows backend checks pass; please test the current interface and Google sign-in on your device. macOS and Linux builds need their platform dependencies and separate validation.

## License

Application source: [MIT](LICENSE). Bundled fonts, icons, dependencies and playback tools retain their own licenses; see [third-party notices](THIRD-PARTY-NOTICES.md).
