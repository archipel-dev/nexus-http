# Nexus HTTP engineering guide

Nexus HTTP is the API client of the Archipel suite, measured against Insomnia / Postman. It runs
standalone (`nexus`) and as an Archipel IDE plugin (`nexus-http`) from the same code.

## Layout

- `crates/nexus-http-core`: headless engine (model, storage, templating, auth, execution,
  import/export, codegen, history, cookies, assertions, runner). No egui.
- `crates/nexus-http`: the views (egui on archipel-kit) and the plugin adapter on `archipel-sdk`.
- `crates/nexus`: the standalone desktop app (eframe) hosting the same views.
- `xtask`: `cargo xtask dist` packages release archives and installers.

## Non-negotiables

- UI only from archipel-kit components, parts and behaviours (https://github.com/archipel-dev/kit,
  read `docs/README.md`, `building-components.md`, `behaviours.md`, `cursors.md`). No ad-hoc
  painting, no private widget or theme modules. Missing kit pieces go to the kit, never into a
  private copy here.
- Every interactive element passes `archipel_kit::testing::conformance`; every screen passes
  `archipel_kit::testing::audit`.
- Golden paths have mouse-only smoke tests (real xdotool input under Xvfb). UI PRs attach
  screenshots taken that way at 1280x800.
- Files are the source of truth: collections are `.http` files plus a `.nexus` folder; settings
  are TOML. No account, no hidden network calls.
- Budgets (CI-checked as they land): cold start < 400 ms, interaction < 16 ms, idle CPU < 1%.
- Clean-room: public documentation and observable behavior only. No Insomnia/Postman code,
  icons, artwork or trademarks in names.

## Required checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commits and PR titles are Conventional Commits (`feat`, `fix`, `perf`, `refactor`, `docs`,
`test`, `chore`, `ci`, `build`; `!` for breaking). Never bump versions by hand: release-please
owns `version` in `Cargo.toml`. Merge `origin/main`, never rebase shared branches.
