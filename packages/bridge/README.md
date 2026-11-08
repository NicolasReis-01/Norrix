# @norrix/bridge

This package owns Norrix's browser/native boundary:

- `src/protocol.ts` is generated from the Rust Tauri command surface;
- `src/canvas.ts` adapts that protocol and `koharu-canvas` to the browser;
- `src/wasm` is ignored derived output produced by `wasm-pack`.

The bridge package builds only its WASM output. Its build and development scripts resolve the repository root from their own file locations, so they do not depend on the calling shell's working directory or package-manager prefix configuration. The root frontend tasks sequence the bridge build before starting or building the consuming Next.js app. During development, `nodemon` rebuilds the bridge when `koharu-canvas` or `norrix-rasterizer` changes. Because the generated glue is imported from this package, Next.js includes it in the Turbopack module graph and refreshes the client after a successful rebuild.

Regenerate the protocol with `cargo run -p norrix-app --bin generate`. Do not edit generated protocol or WASM output by hand.
