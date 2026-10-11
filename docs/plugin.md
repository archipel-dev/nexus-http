# Nexus HTTP as an Archipel IDE plugin

`crates/nexus-http` is an Archipel plugin built on [`archipel-sdk`](https://github.com/archipel-dev/sdk).
The IDE bundles it from this repository; the same views also run in the standalone `nexus` app.

## Consuming it from the IDE

```toml
# IDE app crate
nexus-http = { git = "https://github.com/archipel-dev/nexus-http", tag = "vX.Y.Z" }
```

Register the constructor as a compiled-in plugin factory:

```rust,ignore
registry.add_factory(|| Box::new(nexus_http::NexusHttpPlugin::new()));
```

The IDE and Nexus must resolve the same `archipel-sdk` and `archipel-kit` sources
(`cargo tree -d | rg archipel-kit` shows one).

## Manifest

`crates/nexus-http/archipel-plugin.toml`, embedded in the crate:

- id `dev.archipel.nexus-http`, name "Nexus HTTP", `api_version` = `archipel_sdk::API_VERSION`, kind `rust`
- permissions `network`, `fs.user`, `fs.project`
- `version` is a placeholder in the file; `nexus_http::plugin::manifest()` fills in
  `CARGO_PKG_VERSION` at parse time so it follows release-please.
- `[app] view = "nexus.main"` for standalone app mode.

## Contributions

| Kind | Id | Notes |
|---|---|---|
| Command | `nexus.open` | "HTTP Client"; opens the `nexus.main` editor tab |
| Menu | Tools › HTTP Client | runs `nexus.open` |
| Tool window | `nexus` | shows `nexus.main` |
| Editor view | `nexus.main` | opens `.http` and `.rest` files |

Opening a `.http`/`.rest` file imports its requests into the workspace (once per path) and
reports the count in the status bar; unreadable files raise an error notification.

## Workspace location

`<project>/.nexus` when a project is open (commit it), otherwise `<plugin data dir>/.nexus`.

## Not covered yet

- Secrets: `PluginHost` has no secret-store API in SDK API 1 (only the `secrets` permission),
  so environment secret values stay in Nexus's own storage.
