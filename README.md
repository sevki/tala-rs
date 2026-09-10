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

## Testing

CI (`.github/workflows/ci.yml`) runs two jobs on every push and pull request:

- **`rust`** — `cargo fmt --check`, `cargo clippy -- -D warnings`, and
  `cargo test`, which includes `tests/golden.rs`:
  - a golden-file regression suite that snapshots tala-rs's own layout
    output for the fixtures under `tests/fixtures/*.json` against
    `tests/golden/*.json`. Regenerate goldens after an intentional layout
    change with:

    ```sh
    TALA_UPDATE_GOLDEN=1 cargo test --test golden
    ```

  - structural invariant checks (no unrelated node overlaps, orthogonal
    edge routing, children contained within their parent) that must hold
    for every fixture regardless of exact coordinates.

- **`upstream-compat`** — a small Go harness under `compat/` (pinned to a
  specific `github.com/d2lang/d2` version) that runs the *same* fixtures
  through the real upstream `d2talalayout` engine and asserts the same
  structural invariants. tala-rs isn't expected to produce byte-identical
  coordinates to upstream, but it is expected to satisfy the same layout
  guarantees; this job catches drift from that contract in either
  direction.

New fixtures should be added to `tests/fixtures/` (nodes list parents
before their children); both jobs pick them up automatically.