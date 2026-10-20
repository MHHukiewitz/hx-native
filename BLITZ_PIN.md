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
