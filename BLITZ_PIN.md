# Blitz pin notes (hx-native)

Pinned crate: **`blitz-dom` / `blitz-html` / `blitz-paint` / `blitz-traits` = `0.3.0-beta.4`**.
No Blitz fork. No `unsafe` in transform / damage paths.

## Cached transform scale (beta.4)

`Node::transform()` returns the Affine Blitz stores during `resolve_transforms`.
In **0.3.0-beta.4**, that cached matrix’s **translation components are already
multiplied by the document viewport scale** (device pixels). Linear components
(`a,b,c,d`) are not.

hx-native’s `element_border_box_*` helpers rely on that: when the caller’s
`scale` equals the document viewport scale, the cached Affine is used as-is;
when they differ, only the translation terms are re-based
(`e' = e * scale / doc_scale`).

A Blitz bump that changes this contract (e.g. caching CSS-space transforms)
must update `local_transform_device` in `hx-native/src/damage.rs`.

## APIs used for transform-aware boxes

| API | Role |
|-----|------|
| `Node::transform()` | Cached CSS transform Affine from stylo (device translations at viewport scale) |
| `Node::containing_block()` | Walk the same chain as `absolute_position` |
| `Node::scroll_offset()` | Match `absolute_position` (location − scroll) |
| `Node::scrollable_overflow()` | Local ink inflation (device px at document scale) |
| `Node::final_layout()` | Border-box size and relative `location` |
| `Node::primary_styles()` | Outline / box-shadow for ink pad |

Public helpers: `element_border_box_css`, `element_border_box_device`
(also used by form range geometry and, after #18 rebases, select control rects).

## SVG (issue #23)

`blitz-paint` is built with `default-features = false, features = ["svg"]`.
Crates that move with the Blitz pin and must be bumped together:

| Crate | Pin | Why |
|-------|-----|-----|
| `usvg` | `=0.48.1`, `default-features = false` | Same version Blitz parses with; hx-native uses it only to reject malformed `data:` SVG at validation time (empty fontdb, no text). |
| `anyrender_svg` | `0.15.0` (via `blitz-paint/svg`) | Paints the usvg tree, and decodes SVG `<image>` rasters with `image` 0.25 (features exactly `png`, `jpeg`, `gif`, `webp`; see `cargo tree -e features -i image`). Pulled with **default features**; blitz-paint does not let a downstream crate trim them. |
| `data-url` | `=0.3.2` | Same version blitz-dom decodes `data:` URLs with. |

Lockfile additions for #23 (no removals, no `windows-sys` move): `anyrender_svg 0.15.0`,
`color_quant 1.1.0`, `gif 0.14.2`, `image-webp 0.2.4`, `quick-error 2.0.1`,
`weezl 0.1.12`, `zune-core 0.5.3`, `zune-jpeg 0.5.15`.

Load-bearing facts to re-check at every Blitz bump:

- `NetProvider::fetch` carries no resource kind (image vs stylesheet vs font).
  `DataSvgProvider` (`hx-native/src/svg.rs`) gates on URL scheme + MIME +
  payload; if Blitz starts routing fonts or stylesheets differently, re-run
  `tests/svg.rs` (`data_font_face_is_never_served`,
  `data_css_link_is_ignored_and_never_blocks_paint`).
- `is_noop() == true` keeps `<link>` from becoming pending-critical.
- A loaded image is applied at the next `resolve` (hence `resolve_with_data_images`).
- `blitz_dom::util::FONT_DB` scans system fonts on the first SVG parse; hx-native
  warms it in `Engine::new` (`svg_warm_up`). If Blitz drops the scan or makes it
  configurable, the warm-up can go.
- `draw_svg_image_layer` ignores `background-repeat`; anyrender_svg drops patterns.
- SVG `<image>` (#23 round 2): usvg's default href resolvers read file paths
  and parse nested SVG; hx-native's validation replaces them in its own parse
  and checks every inline `<image>` in the DOM (`check_image_href`). Re-check
  `usvg::parser::image::get_href_data` and the resolver signatures at every bump.
- anyrender_svg draws a red box over an image it cannot decode and decodes on
  every paint; `probe_decode` (a throwaway `<img>` document using blitz-dom's
  `ImageHandler`) must keep matching its decoder (same `image` crate build).
- The header parsers in `src/svg.rs` (PNG IHDR, GIF screen + frames, WebP
  VP8/VP8L/VP8X, JPEG SOF) enforce `DATA_IMAGE_MAX_PIXELS` before any decode.
- SVG children have no layout; `damage::snapshot_bounds` skips them.
