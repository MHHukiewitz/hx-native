# hx-native

**hx-native** brings the htmx way of building interfaces into a native Rust app, with no browser, no JavaScript and no network. You write plain HTML and CSS using the locked dialect attributes `hx-post`, `hx-target`, `hx-swap`, `hx-trigger`, `hx-sync` and `hx-indicator`, plus the engine-side `data-hx-group`, `data-hx-step`, `data-hx-step-for`, `data-hx-applied` and `data-hx-paste-ignore`. Any other `hx-*` / `data-hx-*` attribute is a load error (Mainnet HTML contract §2). A paste into a number box is all or nothing; `data-hx-paste-ignore="$ "` lists characters to strip around the pasted number first, never between two digits, so ` $1,234 ` pastes as `1234` and `3 5` is refused (default: none). A comma, pasted or typed, is accepted only as thousands grouping (`1,234`): a pasted `3,5` is refused, and a typed one leaves the box at its previous value on commit. Listing `,` in `data-hx-paste-ignore` has no effect. Tabbing into a text or number box selects its whole text, so typing replaces it; a click puts the caret where you clicked. Clicking a number box's ▲/▼ while a typed value is still uncommitted steps that value exactly as the arrow keys do (`3.7` ▲ gives `4`), then commits it; the box keeps the focus with the caret at the end. Holding an arrow repeats the step after 400 ms, every 100 ms, and sends one `change` on release, or none when min or max kept the value where it was. A host that forwards key releases sets `EngineConfig::key_releases` so ArrowUp/ArrowDown also send their one `change` on key release. A box greyed while you type in it drops the typed text and shows the game's value. Number boxes show the raw number by default; `data-hx-group="on"` opts a box into `,` thousands grouping while it is not focused (see `docs/REQUIREMENTS.md`). Your Rust code plays the server: it handles typed actions, keeps named, versioned data slices, and returns HTML fragments. hx-native swaps those fragments into a live page and re-lays-out only what changed. It paints the result to a GPU texture with alpha that your app or game composites wherever it wants, and it reports clicks, hover and focus back to you. Rendering is built on [Blitz](https://github.com/DioxusLabs/blitz) and Vello. The same HTML files run unchanged in a real browser with htmx.js, so designers can click through mocks against sample data. Everything is deterministic and testable headless, and every frame comes with a timing report.

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

## Engine-drawn controls

hx-native draws a control only when Blitz cannot paint it at all: the range slider, the scrollbar and the `<select>` ▼ button and drop list. It carries no game-specific styling; theming belongs to your CSS. Every colour and size of these controls comes from the variables below, read as computed on the control (so inline styles, classes, inheritance and a runtime-swapped `<style>` all work). These are the only custom properties the engine reads. Unset, each one falls back to a neutral grey default.

| Variable | Used for | Default |
|---|---|---|
| `--hx-face` | slider thumb, scrollbar buttons and thumb, select button | `#d4d4d4` |
| `--hx-ink` | arrow glyphs, slider focus dots | `#000000` |
| `--hx-bevel-hi` | outer light edge | `#ffffff` |
| `--hx-bevel-lt` | inner light edge | `#e8e8e8` |
| `--hx-bevel-sh` | inner shadow edge, select border (top / left) | `#8c8c8c` |
| `--hx-bevel-dk` | outer dark edge | `#404040` |
| `--hx-track` | scrollbar track | `#e8e8e8` |
| `--hx-track-alt` | if set, the track is a 1 px checker of `--hx-track` and this (`none` = solid) | unset |
| `--hx-select-bg` | select field and drop-list background, highlighted row text | `#ffffff` |
| `--hx-select-ink` | select field and drop-list text | `#000000` |
| `--hx-select-hl` | highlighted drop-list row | `#505050` |
| `--hx-thumb-w` | slider thumb width | `11px` |
| `--hx-groove-h` | slider groove height | `4px` |

**Interim paint contract (#36, #58).** These controls, and the bevel the engine replays on hard-inset typed fields, are painted after Blitz, on top of the page. Until #36 and #58 are fixed, on one surface:
- an opaque overlay (dialog, tooltip, popup) must not cover a slider, a scroll container's 16 px bar strip or a replayed field bevel; put the overlay on another surface or keep it clear;
- `opacity` does not hide them (they paint fully opaque); use `display:none`;
- only scroll containers (`overflow-y: auto | scroll`) clip them; a non-scroll `overflow:hidden` box does not.

Map them from your own design tokens. Mainnet's Win95 look, for example:

```css
:root {
  --hx-face: var(--bg);           /* #c0c0c0 */
  --hx-ink: var(--ink);           /* #000 */
  --hx-bevel-hi: var(--edge-hi);  /* #fff */
  --hx-bevel-lt: var(--edge-lt);  /* #dfdfdf */
  --hx-bevel-sh: var(--edge-sh);  /* #808080 */
  --hx-bevel-dk: var(--edge-dk);  /* #000 */
  --hx-track: var(--bg);
  --hx-track-alt: var(--edge-hi); /* 1 px dither */
  --hx-select-bg: #fff;
  --hx-select-ink: var(--ink);
  --hx-select-hl: #000080;
  --hx-thumb-w: 11px;
  --hx-groove-h: 4px;
}
```

Everything else (text fields, buttons, checkboxes, borders, shadows, `text-align`) is plain CSS painted by Blitz; the engine only corrects two Blitz beta.4 bugs on single-line inputs so your computed CSS shows (`text-align` and the order of multiple inset `box-shadow`s). See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md), "Range slider and number fields (issue #35)".

## Text shadow (issue #57)

Blitz 0.3.0-beta.4 computes `text-shadow` but does not paint it, so the engine draws zero-blur text shadows itself: author CSS, CSS order (first on top), `currentcolor`, alpha, scale and partial repaint included.

Not drawn: shadows with a blur radius (skipped; the others on the same text still paint), **underlines and other text decorations**, **list markers**, and the text inside `<input>` / `<textarea>`. See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md), "Text shadow (issue #57)".

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

The bundled fonts in `assets/fonts/` (DejaVu Sans and DejaVu Sans Bold) are not
under these licences: they are under the Bitstream Vera / Arev licence of the
DejaVu project ([LICENSE-DejaVu.txt](assets/fonts/LICENSE-DejaVu.txt),
upstream text in [LICENSE-DejaVu-2.37.txt](assets/fonts/LICENSE-DejaVu-2.37.txt)).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
