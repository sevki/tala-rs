# tala-rs

`tala-rs` is a compact Rust port of the core TALA ideas described in D2's
open-source release:

- deterministic multi-seed layout attempts
- orthogonal edge routing
- fixed-position nodes
- container-aware sizing for nested nodes

The implementation in this repository intentionally starts with a small,
dependency-free Rust library surface:

- `default_layout(&mut Graph)`
- `layout(&mut Graph, Option<&Options>)`
- `route_edges(&mut Graph)`

The data model is also intentionally small and self-contained:

- `Graph`
- `Node`
- `Edge`
- `Options`

This is not a line-for-line translation of the full Go implementation under
`d2layouts/d2talalayout`, but it preserves the seed selection model and
orthogonal layout/routing behavior that make TALA distinctive.