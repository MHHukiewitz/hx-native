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

## Engine-drawn `text-shadow` (issue #57, pin-sensitive)

Blitz 0.3.0-beta.4 computes `text-shadow` but never paints it.
`hx-native/src/text_shadow.rs` draws zero-blur shadows in the `PaintScene`
wrapper (`ShadowScene`) right before the glyph run they belong to. It depends
on these beta.4 internals; a Blitz bump must re-check each one, and drop the
module once Blitz paints `text-shadow` itself:

| Assumption | Where in Blitz beta.4 |
|-----|------|
| `draw_glyphs` gets the run's `FontData`, `font_size` and parley's positioned glyph `x`/`y` unchanged (the run key) | `blitz-paint/src/text.rs` `stroke_text` |
| Inline-root transform = `parent * translate(location) * node.transform()`, then the node's own scroll offset applied **twice** (`cx.transform.then_translate(-scroll)` and `content_position - scroll`), then `translate(padding + border + block_offset)` | `blitz-paint/src/render.rs` `render_element`, `draw_inline_layout` |
| Each run's brush is `TextBrush { id }` of the run's own node (its computed `text-shadow` and `color`) | `blitz-dom/src/node/text.rs` |
| Text inside `<input>` / `<textarea>` editors is not an inline layout, so it gets no shadow | `render.rs` `draw_text_input_text` |

A run whose origin does not match within 0.5 device px paints without a
shadow (fail safe). Shadows with a blur radius are skipped. Damage
(`damage.rs` `ink_box_device`) grows each element's ink by the offsets (and
1.5× blur) of the shadows on the runs it lays out.

Known, unrelated to #57: in `PaintMode::Shifted`, synthetic italic (the run's
skew `glyph_transform`) repaints differently from a full paint (172 px on a
20 px "Hi"), most likely because `CullScene` shifts `glyph_transform` as
well as `transform`. Exact is unaffected. Not fixed here.
