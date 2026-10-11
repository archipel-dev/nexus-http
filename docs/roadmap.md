# Roadmap

Nexus HTTP is the API client of the Archipel suite, measured against Insomnia (and Postman). The
charter: faster (budgets enforced in CI), consistent (one design system, archipel-kit), proven
interaction quality, files as the source of truth, one suite (standalone and IDE plugin), optional
local-only AI, and honest parity with evidence. See the [parity table](parity/insomnia.md).

All items link their GitHub issue. Epic: [Milestone 1: v0.1.0 (#30)](https://github.com/archipel-dev/nexus-http/issues/30).

## v0.1.0 (Milestone 1)

| Item | Issue |
|---|---|
| Workspace move from the Archipel IDE tree (`crates/nexus-http-core`, `crates/nexus-http`, `crates/nexus`) | [#3](https://github.com/archipel-dev/nexus-http/issues/3) |
| CI and release (fmt, clippy, tests; release-please; Linux tar.gz/.deb, macOS, Windows installers) | [#7](https://github.com/archipel-dev/nexus-http/issues/7) |
| All views on archipel-kit | [#4](https://github.com/archipel-dev/nexus-http/issues/4) |
| Kit conformance and audit tests for every view | [#5](https://github.com/archipel-dev/nexus-http/issues/5) |
| Mouse-only Xvfb smoke tests per golden path, with screenshots | [#6](https://github.com/archipel-dev/nexus-http/issues/6) |
| README screenshots after the kit port | [#10](https://github.com/archipel-dev/nexus-http/issues/10) |
| Plugin adapter on archipel-sdk (blocked on the SDK) | [#8](https://github.com/archipel-dev/nexus-http/issues/8) |
| Archipel IDE bundles Nexus from a release tag | [#9](https://github.com/archipel-dev/nexus-http/issues/9) |
| README, roadmap and parity docs | this PR |

## v0.2+

### Daily-use blockers

| Item | Issue |
|---|---|
| Collections as `.http` files plus `.nexus` folder, readable git diffs | [#11](https://github.com/archipel-dev/nexus-http/issues/11) |
| Settings in TOML; guard test for no hidden network calls | [#12](https://github.com/archipel-dev/nexus-http/issues/12) |
| Cookie jar manager UI | [#26](https://github.com/archipel-dev/nexus-http/issues/26) |
| Export to `.http`, Postman, OpenAPI, HAR | [#24](https://github.com/archipel-dev/nexus-http/issues/24) |
| Collection runner UI parity | [#18](https://github.com/archipel-dev/nexus-http/issues/18) |

### Speed budgets in CI

| Budget | Issue |
|---|---|
| Cold start < 400 ms | [#13](https://github.com/archipel-dev/nexus-http/issues/13) |
| Every interaction < 16 ms (frame time) | [#14](https://github.com/archipel-dev/nexus-http/issues/14) |
| Idle CPU < 1% | [#15](https://github.com/archipel-dev/nexus-http/issues/15) |
| Memory footprint | [#16](https://github.com/archipel-dev/nexus-http/issues/16) |

### Consistency

Covered by [#4](https://github.com/archipel-dev/nexus-http/issues/4) (kit port),
[#5](https://github.com/archipel-dev/nexus-http/issues/5) (conformance and audit) and
[#6](https://github.com/archipel-dev/nexus-http/issues/6) (mouse-only smoke tests). New screens added
after v0.1.0 follow the same rules (see [AGENTS.md](../AGENTS.md)).

### Parity gaps

| Item | Issue |
|---|---|
| Finish gRPC (from the archived WIP branch) | [#17](https://github.com/archipel-dev/nexus-http/issues/17) |
| Headless CLI | [#19](https://github.com/archipel-dev/nexus-http/issues/19) |
| Mock servers | [#20](https://github.com/archipel-dev/nexus-http/issues/20) |
| Plugin system (custom template tags, hooks) | [#21](https://github.com/archipel-dev/nexus-http/issues/21) |
| Git-native sync and collaboration UX | [#22](https://github.com/archipel-dev/nexus-http/issues/22) |
| OpenAPI design: edit and lint specs | [#23](https://github.com/archipel-dev/nexus-http/issues/23) |
| More auth types (OAuth 1.0, NTLM, Hawk, ...) | [#25](https://github.com/archipel-dev/nexus-http/issues/25) |
| XPath response filtering | [#31](https://github.com/archipel-dev/nexus-http/issues/31) |

### Beyond

| Item | Issue |
|---|---|
| Run an HTTP call from a gutter icon next to its route (IDE) | [#27](https://github.com/archipel-dev/nexus-http/issues/27) |
| Typed clients and models from responses and collections | [#28](https://github.com/archipel-dev/nexus-http/issues/28) |
| Optional AI with local models only | [#29](https://github.com/archipel-dev/nexus-http/issues/29) |
| Files as truth (`.http` in the repo) | [#11](https://github.com/archipel-dev/nexus-http/issues/11) |
