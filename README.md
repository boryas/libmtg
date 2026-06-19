# libmtg

A generic, reusable rules engine for Magic: The Gathering, plus simulation apps
built on top of it.

This repository is a Cargo workspace. The headline crate is **`mtg-engine`** — a
standalone engine with a public state/decision API (no game-specific strategy,
no database, no UI). The other crates are consumers and tools built on it.

## Crates

| Crate | What it is |
|-------|------------|
| [`mtg-engine`](crates/mtg-engine) | The reusable engine: cards, zones, mana, the stack, replacement/trigger infrastructure, and a public API for driving game state and decisions. No dependency on the other crates. |
| [`doomsday`](crates/doomsday) | Doomsday-specific strategy, planner, objective, and scenario generation built on `mtg-engine`. |
| [`decklist`](crates/decklist) | Portable text/URL decklists, independent of the engine and any database. Native-only `fetch` feature resolves Moxfield / MTGGoldfish URLs with an on-disk cache. |
| [`dd-goldfish`](crates/dd-goldfish) | Monte-Carlo goldfish simulator for Doomsday (CLI + wasm web frontend). |
| [`pilegen`](crates/pilegen) | Doomsday pile-scenario generator (wasm web frontend). |

## Using the engine in your own project

`mtg-engine` lives inside this workspace but is consumed like any other crate.
You don't need crates.io — point Cargo at this repo via a git dependency:

```toml
[dependencies]
mtg-engine = { git = "https://github.com/boryas/libmtg", package = "mtg-engine" }
```

Cargo builds only `mtg-engine` and its dependencies (`rand`, `serde`); the
Doomsday apps in this workspace are never compiled by your project. Pin to a
`tag`, `rev`, or `branch` for reproducibility.

## Build & test

```sh
cargo build
cargo test
```

## Web frontends

`deploy.sh` builds the `dd-goldfish` and `pilegen` wasm frontends with
`wasm-pack` and rsyncs them (with `index.html` / `dd-goldfish.html`) to the
static host.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
