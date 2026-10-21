# Engine test fixtures

These HTML shells are **engine-owned test data** for `hx_native` headless
acceptance tests. They are **not** Designer mocks and must not be treated as
the Canvas/Mainnet chrome contract.

The time-strip-like shell (`time_strip.html`) exercises the locked attribute
dialect (`hx-post`, `hx-target`, `hx-swap`, `hx-trigger`, `hx-indicator`,
`hx-sync` only — HTML contract §2) against a `time` slice section and a few
buttons.

`tstrip_shell.html` + `chips.html` mirror the Designer time-strip shape
(Mainnet PR #686 sample): section swaps into `#tstrip-chips`, and
`#tstrip-ind` is a **sibling** outside the fragment so fragment validation
must resolve indicators against the post-swap document.

`settings34_advanced.html` (issue #26, scrolling) is a faithful extract of
Mainnet `docs/design/mocks/target/34-settings.html` at `c0194c06`: the 480x356
Settings dialog on a 1280x720 surface, with the rules it uses from
`desk98.css` / `_tokens.css` inlined and the Advanced tab
(`samples/pause.settings/advanced.html`, also kept verbatim as
`settings34_advanced_panel.html` for swap tests) in `#set-panel`.
`#set-panel-advanced.panel.scroll` is the `overflow-y:auto` container; its
rows are taller than the panel. Font is bundled DejaVu Sans.
