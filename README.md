# hx-native

An in-process, offline, HTMX-style fragment engine for Rust game UIs. You write HTML/CSS chrome with the htmx attribute dialect (`hx-post`, `hx-target`, `hx-swap`, `hx-trigger`). The host game supplies typed actions, named versioned data slices, and finished HTML fragments (for example rendered with askama). `hx-native` swaps only the dirty fragments into a live DOM, restyles and relayouts just that part ([Blitz](https://github.com/DioxusLabs/blitz) + Stylo + Taffy), and paints to an RGBA buffer or texture with alpha that the game composites ([Vello](https://github.com/linebender/vello)). It reports hover, press, focus, clicks, keys and actions back to the host. There is no network and no JavaScript in the game path, and it runs headless and deterministic in tests. The same HTML runs in a browser with real htmx.js for design mocks. Its first host is the game Mainnet.

**Status:** requirements locked 2026-10-19 ([docs/REQUIREMENTS.md](docs/REQUIREMENTS.md)). No crate code yet. Layer 1 (time-strip spike, gate G1) is tracked in issue #1.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
