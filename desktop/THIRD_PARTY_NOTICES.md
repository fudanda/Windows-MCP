# File transfer components

- **Dufs 0.46.0** — https://github.com/sigoden/dufs/tree/v0.46.0 — MIT or Apache-2.0. The bundled binary uses the MIT license; its full text is included in `third-party/dufs-LICENSE-MIT.txt` and beside `file-server/dufs.exe`. The official Windows release ZIP is pinned and SHA-256 verified by `scripts/build-file-server.ps1`.
- **Uppy Core, Dashboard, XHR Upload, Locales** — https://github.com/transloadit/uppy — MIT. Handles browser file selection, upload queue, progress, cancellation and localization.
- **@noble/hashes** — https://github.com/paulmillr/noble-hashes — MIT. Computes SHA-256 incrementally in browsers, including LAN HTTP origins where Web Crypto is unavailable.
- **Axum, Tokio, Reqwest, SHA-2** — MIT / Apache-2.0 as distributed by their respective crates. Used for the authenticated gateway, streaming, task integration and integrity checks.
- **async-deflate-zip 0.4.0** — https://crates.io/crates/async-deflate-zip/0.4.0 — MIT or Apache-2.0. Streams ZIP exports with explicit directory entries through bounded buffers. Also used by Dufs; the manager uses its directory-entry API to preserve empty folders. The MIT license is included in `third-party/async-deflate-zip-LICENSE-MIT.txt` and bundled with the app.
- **HTTPX 0.28.1** (BSD-3-Clause) and **Rich 14** (MIT) — used by the standalone Python transfer command for HTTP streaming and terminal progress. UV resolves these isolated script dependencies; no desktop backend dependencies are imported on the client.

Exact JavaScript and Rust dependency versions are recorded in `package-lock.json` and `src-tauri/Cargo.lock`.
