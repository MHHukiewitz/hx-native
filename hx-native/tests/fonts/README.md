# Test fonts (issue #28)

DejaVu Sans Bold moved to `assets/fonts/DejaVuSans-Bold.ttf` in issue #56:
it is now bundled into the library as the fallback bold face. It is DejaVu
Sans Bold from the upstream DejaVu 2.37
release (`dejavu-fonts-ttf-2.37.tar.bz2`,
https://github.com/dejavu-fonts/dejavu-fonts/releases/tag/version_2_37),
unmodified. sha256:
`e6476c1b80502924294eed40894c5b18e06c181444ca953e5334262df9c27724`.

It is under the same Bitstream Vera / DejaVu licence as the bundled
`assets/fonts/DejaVuSans.ttf` (see `LICENSE-DejaVu.txt` here, the upstream
`LICENSE` file, and `assets/fonts/LICENSE-DejaVu.txt`). The tests keep
using it from `assets/fonts/` as a host face.

`DejaVuSans-subset-collection1.woff2` (5628 bytes) is a WOFF2-compressed
one-face TrueType collection (`ttcf`) made from the bundled
`assets/fonts/DejaVuSans.ttf` (sha256
`ae7b7855e115a5966d8b1b3f80f254ccc117ec86f9965e202ee2940453837280`):
`pyftsubset DejaVuSans.ttf --text="Collection" --name-IDs='*' --name-legacy
--name-languages='*' --notdef-outline`, then fontTools
`TTCollection` with that one face, then `woff2_compress`. It is a modified
DejaVu font under the same licence (`LICENSE-DejaVu.txt`): the Bitstream
Vera and Arev terms allow modification and redistribution with the notice
kept, as long as the font is not sold by itself and its names do not
contain "Bitstream", "Vera", "Arev" or "Tavmjong Bah"; its family name
stays "DejaVu Sans". Used only by `woff2_collection_is_rejected` in
`tests/host_fonts.rs`, which checks that the `ttcf` collection check runs
on the decoded bytes.
