# hx-native

**hx-native** brings the htmx way of building interfaces into a native Rust app, with no browser, no JavaScript and no network. You write plain HTML and CSS using the locked dialect attributes `hx-post`, `hx-target`, `hx-swap`, `hx-trigger`, `hx-sync` and `hx-indicator`, plus the engine-side `data-hx-group`, `data-hx-step`, `data-hx-step-for`, `data-hx-applied` and `data-hx-paste-ignore`. Any other `hx-*` / `data-hx-*` attribute is a load error (Mainnet HTML contract §2). A paste into a number box is all or nothing; `data-hx-paste-ignore="$ "` lists characters to strip from pasted text first (default: none). A pasted comma is accepted only as thousands grouping (`1,234`), so `3,5` is refused; listing `,` in `data-hx-paste-ignore` has no effect. Number boxes show the raw number by default; `data-hx-group="on"` opts a box into `,` thousands grouping while it is not focused (see `docs/REQUIREMENTS.md`). Your Rust code plays the server: it handles typed actions, keeps named, versioned data slices, and returns HTML fragments. hx-native swaps those fragments into a live page and re-lays-out only what changed. It paints the result to a GPU texture with alpha that your app or game composites wherever it wants, and it reports clicks, hover and focus back to you. Rendering is built on [Blitz](https://github.com/DioxusLabs/blitz) and Vello. The same HTML files run unchanged in a real browser with htmx.js, so designers can click through mocks against sample data. Everything is deterministic and testable headless, and every frame comes with a timing report.

**Status:** requirements locked 2026-10-19 ([docs/REQUIREMENTS.md](docs/REQUIREMENTS.md)). Crate `hx-native` (lib `hx_native`) implements the Layer 1 headless engine (dialect, triggers, pending/`hx-sync`, `vello_cpu` paint). Mainnet G1 upload + chrome wiring are tracked separately in [issue #1](https://github.com/MHHukiewitz/hx-native/issues/1).

## Fonts

The engine never loads system fonts or CSS `@font-face` files. It bundles
DejaVu Sans at 400 and 700 (normal style) as the fallback for every unknown
family and for `sans-serif`, `serif`, `monospace` and `system-ui`. Blitz
does not paint synthetic bold, so a weight only paints bold if a real face for
it is registered.

Hosts should pass their own regular **and** bold faces in
`EngineConfig::fonts`, under the family name their CSS uses:

```rust
use std::sync::Arc;
use hx_native::{EngineConfig, FontFace, FontStyle};

const REGULAR: &[u8] = include_bytes!("assets/fonts/my_ui.ttf");
const BOLD: &[u8] = include_bytes!("assets/fonts/my_ui_bold.ttf");

let config = EngineConfig {
    fonts: vec![
        FontFace::new("My UI", 400, FontStyle::Normal, Arc::from(REGULAR)),
        FontFace::new("My UI", 700, FontStyle::Normal, Arc::from(BOLD)),
    ],
    ..EngineConfig::default()
};
```

With only the regular face, `font-weight: 700` in that family paints the
regular face. With neither, text falls back to DejaVu Sans, and bold then
paints in DejaVu Sans Bold. A host face named `"DejaVu Sans"` at 400 or 700
normal is refused (`EngineError::InvalidFont`), since the bundled face would
shadow it.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
