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
