# xmip-core-path-index

The `index` path technology, a technology of
[xmip-core-path](https://github.com/IlleNilsson/xmip-core-path). It carries
`IndexLanguage`, the `PathLanguage` for the language `index`, fields split on
the delimiter it is configured with: an index compiles once, a Stream's lines
are found once per Message for every index reading it, and a write edits the
text in place. Promote reads through it, demote writes through it to produce a
new Stream (ADR-0013), route and process read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
