# MSRV investigation (PR #2 follow-up)

**Mainnet lock:** Rust `1.85.0` (`rust-version = "1.85"`).  
**Locked engine pin:** Blitz `=0.3.0-beta.4` (REQUIREMENTS / issue #1).

## What forces >1.85

`cargo +1.85.0 check -p hx-native` refuses before compile:

| Package | `rust-version` field |
|---------|----------------------|
| `blitz-dom` / `blitz-html` / `blitz-paint` / `blitz-traits` / `stylo_taffy` `0.3.0-beta.4` | **1.91.0** |
| Many transitive crates (`icu_*` 2.3, `parley` 0.12, `vello_cpu` 0.3, `fearless_simd` 0.7, …) | 1.86–1.89 |

Those Blitz crates also use **edition = "2024"**.

## (a) Pin transitive deps, keep Blitz `=0.3.0-beta.4`

**Does not work for Mainnet 1.85.**

1. Cargo’s `rust-version` gate on `blitz-*@0.3.0-beta.4` alone requires **1.91** unless every consumer passes `--ignore-rust-version`.
2. Even with `--ignore-rust-version`, `cargo +1.85.0 check` fails on real language features in hard deps of Blitz (not optional):
   - `icu_collections` / `icu_locale_core` **2.3** → unstable `let`-chains, `unsigned_is_multiple_of`
   - Blitz depends on `icu_* ^2.3` — cannot pin below 2.3 without forking Blitz
3. Bisection with `--ignore-rust-version` on this tree:
   - **1.85 / 1.86 / 1.87** — fail (icu)
   - **1.88** — fail (`fearless_simd` / vello stack)
   - **1.89** — **compiles and lib tests pass** (metadata still says 1.91)
   - **1.91** — compiles without `--ignore-rust-version` (declared MSRV)

So the practical floor for the locked Blitz pin is **1.89** (ignore) or **1.91** (honest cargo gate). Not 1.85.

## (b) Older Blitz that builds on 1.85

| Blitz | Declared MSRV | Notes vs locked spec |
|-------|---------------|----------------------|
| `0.3.0-beta.4` (locked) | 1.91.0 | Required by REQUIREMENTS / issue #1 |
| `0.3.0-beta.2` / `beta.1` / `alpha.6` | 1.89.0 | Still ≫ 1.85; older API surface |
| `0.2.4` | **1.86.0** | Still > Mainnet 1.85.0; pre-beta.4 APIs |
| `0.1.4` | **1.85.1** | Closest — still not 1.85.0; large feature gap vs beta.4 (traits, mutator, paint, StyleThreading, HtmlProvider, …) |

No published Blitz release both matches the locked `=0.3.0-beta.4` contract and has MSRV ≤ 1.85.0.

## (c) Minimum toolchain if neither (a) nor (b) works

| Mode | Minimum rustc |
|------|----------------|
| Declared / CI without ignore | **1.91.0** |
| Actual compile of this dependency tree | **1.89.0** (`--ignore-rust-version`) |
| Mainnet lock | **1.85.0** — **incompatible** with locked Blitz pin |

**Decision for this crate (until Mike unlocks):** keep `rust-version = "1.91"` and CI on **1.91**. Do **not** silently retarget to 1.85. Options for Mainnet integration later: bump Mainnet toolchain (≥1.89, preferably 1.91), or Mike re-opens the Blitz version lock.
