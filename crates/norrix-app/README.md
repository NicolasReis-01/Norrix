# norrix-app

`norrix-app` owns Norrix's Tauri-managed application state, command API,
project lifecycle, processing jobs, typed channels, and agent host. It uses
`norrix-desktop` to prepare and publish page frames for the browser canvas.

Rust command signatures are the authoritative frontend contract:

```powershell
cargo run -p norrix-app --bin generate
```
