# hx-native

**hx-native** brings the htmx way of building interfaces into a native Rust app, with no browser, no JavaScript and no network. You write plain HTML and CSS using the familiar `hx-post`, `hx-target`, `hx-swap` and `hx-trigger` attributes. Your Rust code plays the server: it handles typed actions, keeps named, versioned data slices, and returns HTML fragments. hx-native swaps those fragments into a live page and re-lays-out only what changed. It paints the result to a GPU texture with alpha that your app or game composites wherever it wants, and it reports clicks, hover and focus back to you. Rendering is built on [Blitz](https://github.com/DioxusLabs/blitz) and Vello. The same HTML files run unchanged in a real browser with htmx.js, so designers can click through mocks against sample data. Everything is deterministic and testable headless, and every frame comes with a timing report.

**Status:** requirements locked 2026-10-19 ([docs/REQUIREMENTS.md](docs/REQUIREMENTS.md)). No crate code yet. Layer 1 (time-strip spike, gate G1) is tracked in issue #1.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
