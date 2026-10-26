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
| Anonymous blocks (mixed inline and block children) and `::before` / `::after` boxes are not in the DOM `children` list; they are found through `Node::anonymous_blocks`, `before()` and `after()` | `blitz-dom/src/layout/construct.rs` `create_anonymous_block`, `node.rs` |
| Text inside `<input>` / `<textarea>` editors is not an inline layout, so it gets no shadow | `render.rs` `draw_text_input_text` |

A run whose origin does not match within 0.5 device px paints without a
shadow (fail safe). Shadows with a blur radius are skipped. Damage
(`damage.rs` `ink_box_device`) grows each element's ink by the offsets (and
1.5× blur) of the shadows on the runs it lays out.

Known, unrelated to #57: in `PaintMode::Shifted`, synthetic italic (the run's
skew `glyph_transform`) repaints differently from a full paint (172 px on a
20 px "Hi"), most likely because `CullScene` shifts `glyph_transform` as
well as `transform`. Exact is unaffected. Not fixed here.

## Tiled gradient fills (pin-sensitive, #67)

`hx-native/src/tile_coalesce.rs` (`TileScene`, wrapped around every
`paint_scene` call in `paint.rs`) relies on how **0.3.0-beta.4**
`blitz-paint` `render/background.rs::draw_gradient_layer` paints a gradient
whose `background-size` is smaller than the box:

- one `fill` per tile, row-major (rows outer, columns inner), with no other
  command in between, inside the background clip layer;
- shape `Rect(0, 0, tile_w, tile_h)`, the same `Gradient` and brush transform
  for every tile, transform `base.then_translate(pos).then_translate(col * w,
  row * h)`;
- conic and radial position and size live in the brush transform, the
  gradient itself sits at the origin (so the tile cache key includes the
  brush transform and the tile size);
- more than 500 tiles: Blitz draws nothing (#55), so there is no run.

A bump that changes the loop order, the shape or the transform layout does
not break painting (an unrecognised run is replayed tile by tile), but loses
the merge: `merged_runs_count_and_fallbacks_at_scale_1` and
`dither_box_reaches_the_renderer_as_one_merged_fill` in
`hx-native/tests/tile_coalesce_67.rs` fail first. A bump that draws tiles
differently (e.g. one fill with an extended brush) makes the pass a no-op;
remove it then.

## Zero-blur inset box-shadow (pin-sensitive, #123)

`hx-native/src/inset_shadow.rs` (`InsetScene`, between `ShadowScene` and
`TileScene` in `paint.rs`) relies on how **0.3.0-beta.4** `blitz-paint`
`render/box_shadow.rs::draw_inset_box_shadow` paints one inset layer:

- `push_layer(NonZero, Mix::Normal, 1.0, T, padding_box_path)`, `fill` of the
  same path with the shadow colour, `push_layer(NonZero, Compose::DestOut,
  1.0, T, padding_box_path)`, `draw_box_shadow` of the hole in white, then two
  `pop_layer`s, with nothing in between;
- the padding box path of a box with sharp corners is a `MoveTo` and three
  `LineTo`s (`kurbo_css` `insert_point`);
- the hole is `padding_box.inflate(-spread * scale)`, moved by the offset in
  **CSS px, not device px** (a 1 px bevel moves 1 device px at scale 2), with
  the averaged corner radius and `blur * scale` as the standard deviation;
- a transparent layer ends the loop (`return`, not `continue`).

vello_cpu 0.3 draws the hole with its blurred rounded rect shader (`powf` per
pixel) even at zero blur; at zero blur that is a hard step at each pixel
centre. With no blur and no radius, under a pure translation, with both
rects on whole device px, `InsetScene` paints the padding box minus the hole
as one even-odd fill (inside the outer layer only for a translucent colour).
Everything else is replayed unchanged.

A bump that changes the command order or the path makes the fast path a
no-op (the layer replays): `whole_pixel_layers_take_the_fast_path_and_the_rest_replay`
and `a_bevelled_panel_paints_every_layer_as_one_fill` in
`hx-native/tests/inset_shadow_123.rs` fail first. A bump that scales the
offset changes the pixels of both paths alike; a bump that paints zero-blur
inset shadows as fills makes the pass redundant; remove it then.

## Radio baseline and ring scale (pin-sensitive, #116)

`hx-native/src/radio.rs` works around two behaviours of **0.3.0-beta.4**:

- **Baseline.** `blitz-dom` gives a radio no baseline (it is not in
  `is_replaced_element`, so it lays out as a block and parley gets an inline
  box with `baseline == None`), so the *margin-box* bottom sits on the line
  baseline. Chrome puts the *border-box* bottom there. `RadioShifts` undoes its
  previous shift, runs `BaseDocument::resolve`, then moves each in-flow inline
  radio down by its `margin-bottom` in both `unrounded_layout` and
  `final_layout`. A UA `vertical-align` shim is not an option: Blitz pushes
  `InlineBox.vertical_align` to parley unscaled, so it breaks at scale != 1.
  Radios with `vertical-align: middle | text-top | text-bottom | top | bottom`
  (or `alignment-baseline`/`baseline-shift` keywords) align by their margin box
  in both engines and are not shifted.
- **Ring scale.** `blitz-paint` `render/form_controls.rs::draw_input` draws the
  ring, gap and dot as circles of radius `8k`, `6k`, `4k` with
  `k = (min_device_dim - 4) / 16`; the `4` is device px. `RadioRings` (hooked
  into `ShadowScene::fill` in `text_shadow.rs`) matches those circles by centre
  and radius and repaints them with `k = (min_css_dim - 4) / 16 * scale`.

A bump that gives radios a baseline, scales the `4`, or changes the circle
radii makes the matching a no-op (unmatched circles paint unchanged); remove
the pass then. Tests: `hx-native/tests/radio_baseline_116.rs`.

Known, unrelated to #116: in `PaintMode::Shifted`, `CullScene` compares the
*shifted* shape bounds with the *unshifted* clip, so small shapes (a radio's
dot at scale 1.5) can be culled in a partial repaint. Exact is unaffected. Not
fixed here.
