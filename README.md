# Nexus HTTP

The API client of the [Archipel](https://github.com/archipel-dev) suite. A native, offline,
account-free alternative to Insomnia and Postman, measured against them. It runs standalone
(`nexus`) and as an Archipel IDE plugin (`nexus-http`) from the same code.

> **Status:** the code is being moved into this repo from the Archipel IDE tree. The first release,
> v0.1.0, is tracked in [Milestone 1](https://github.com/archipel-dev/nexus-http/issues/30).
> Everything below says what is planned or tested; see the [parity table](docs/parity/insomnia.md)
> for evidence per feature.

## Highlights

- **Fast, by budget.** Targets: cold start under 400 ms, every interaction under 16 ms, idle CPU
  under 1%, memory a fraction of an Electron app. They become CI checks
  ([#13](https://github.com/archipel-dev/nexus-http/issues/13),
  [#14](https://github.com/archipel-dev/nexus-http/issues/14),
  [#15](https://github.com/archipel-dev/nexus-http/issues/15),
  [#16](https://github.com/archipel-dev/nexus-http/issues/16)).
- **One design system.** All UI is built from `archipel-kit`; every interactive element and screen is
  covered by kit conformance and audit tests, and golden paths have mouse-only Xvfb smoke tests.
- **Files are the source of truth.** Collections are `.http` files plus a `.nexus` folder in your
  repo, settings are TOML. No account, works offline, no cloud lock-in
  ([#11](https://github.com/archipel-dev/nexus-http/issues/11)).
- **One suite.** The same code is the standalone app and an Archipel IDE plugin, with a planned
  gutter icon to run a call next to the route that serves it
  ([#27](https://github.com/archipel-dev/nexus-http/issues/27)) and typed client/model generation
  ([#28](https://github.com/archipel-dev/nexus-http/issues/28)).
- **Optional AI, local only.** Off by default, local models, never a hidden network call
  ([#29](https://github.com/archipel-dev/nexus-http/issues/29)).
- **Honest parity.** Every claim in the [parity table](docs/parity/insomnia.md) points at a test or a
  screenshot, or says it is missing and links the issue.

Engine features (tested in the headless core): HTTP requests with all common body types,
environments and `{{ variables }}`, `{% template tags %}` with response chaining, Basic / Bearer /
API key / Digest / OAuth 2.0 / AWS SigV4 auth, cookies, GraphQL, WebSocket, Server-Sent Events,
import (curl, `.http`, Postman, Insomnia, OpenAPI 3), code generation for seven targets, history,
JSONPath filtering, assertions and a collection runner.

## Install

Binaries are **available from v0.1.0** on the
[GitHub releases page](https://github.com/archipel-dev/nexus-http/releases):

- Linux: `tar.gz` and `.deb`
- macOS: archive/installer
- Windows: installer

Until then, build from source.

## Run

```bash
nexus [DIR]    # open the collection in DIR (stored in DIR/.nexus)
nexus --demo   # seeded demo workspace with a local demo API
```

## Build from source

```bash
cargo build --release -p nexus
cargo run -p nexus -- --demo
```

Release archives and installers are produced by `cargo xtask dist`. Checks run before every PR:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workspace layout

| Path | Purpose |
|---|---|
| `crates/nexus-http-core` | Headless engine: model, storage, templating, auth, execution, import/export, code generation, history, cookies, assertions, runner. No egui. |
| `crates/nexus-http` | Views (egui on archipel-kit) and the plugin adapter on `archipel-sdk`. |
| `crates/nexus` | The standalone desktop app hosting the same views. |
| `xtask` | `cargo xtask dist`: release archives and installers. |

## Screenshots

<!-- screenshots: added after the kit port -->

Screenshots will be added once the views are ported to archipel-kit
([#10](https://github.com/archipel-dev/nexus-http/issues/10)).

## Documentation

- [Roadmap](docs/roadmap.md)
- [Insomnia parity table](docs/parity/insomnia.md)
- [Engineering guide (AGENTS.md)](AGENTS.md)

## Clean-room

Nexus is built from public documentation and observable behavior only. It contains no Insomnia or
Postman code, icons or artwork. Those products are named in the docs only as the comparison target.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
