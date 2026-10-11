# Parity with Insomnia

Insomnia's public documentation is the source for what the comparison target offers. Nexus is a
clean-room implementation: public docs and observed behavior only.

**Status values**

- **Done**: tested, with a test or screenshot as evidence.
- **Done (engine; UI evidence pending)**: the headless engine is tested, but no UI screenshot or
  mouse-only smoke test exists yet.
- **Partial**: some of the feature is tested; the gap is described and linked.
- **Missing**: not implemented; the issue is linked.

**Evidence** is `path::test_name`. Paths follow the workspace layout of this repo
(`crates/nexus-http-core/...`, `crates/nexus-http/src/...`); the tests are being moved from the
Archipel IDE tree (`core/` becomes `crates/nexus-http-core/`, `src/` becomes
`crates/nexus-http/src/`), so they land with [#3](https://github.com/archipel-dev/nexus-http/issues/3).
Until the UI is ported to archipel-kit ([#4](https://github.com/archipel-dev/nexus-http/issues/4)) and
the mouse-only smoke tests exist ([#6](https://github.com/archipel-dev/nexus-http/issues/6)), no row
is marked plain **Done** and UI evidence is "screenshot pending".

Abbreviations: `S` = `crates/nexus-http-core/tests/server.rs`, `C` = `crates/nexus-http-core/src`.

Source URLs: pages cited below returned HTTP 200 when this table was written, except the plugins
and mock-servers pages, which are listed in the docs navigation but returned 404 to an automated
fetch and should be re-checked. Where no specific page is cited, the docs root
<https://docs.insomnia.rest/> is the source and the exact page is to be confirmed.

## Requests, methods and bodies

Source: <https://docs.insomnia.rest/insomnia/requests>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Requests | Standard and custom methods, query params, headers | Done (engine; UI evidence pending) | `S::sends_method_query_headers_and_json_body`, `C/model.rs::method_round_trips_and_preserves_custom`; screenshot pending |
| Requests | Redirect handling | Done (engine; UI evidence pending) | `S::follows_redirects_with_method_rules` |
| Requests | Timeouts, connection errors, response truncation, cancel | Done (engine; UI evidence pending) | `S::reports_timeouts_connection_errors_and_truncation`, `S::cancels_before_sending` |
| Bodies | JSON, XML, text, form URL-encoded, multipart, binary, GraphQL | Done (engine; UI evidence pending) | `C/exec.rs::encodes_bodies`, `C/model.rs::body_serializes_with_kind_tag`; screenshot pending |
| Requests | Folder-level inheritance of headers/params/auth | Done (engine; UI evidence pending) | `C/exec.rs::prepares_url_params_headers_and_folder_inheritance` |
| Responses | Status, headers, timings and timeline, size, body kind detection | Done (engine; UI evidence pending) | `C/response.rs::detects_body_kinds`, `C/response.rs::pretty_prints_json_and_formats_sizes`; screenshot pending |

## Environments and variables

Source: <https://docs.insomnia.rest/insomnia/environment-variables>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Environments | Base and sub environments, folder variables, nested values and paths | Done (engine; UI evidence pending) | `C/template.rs::merges_environments_and_folders`, `C/template.rs::substitutes_variables_and_paths`, `C/template.rs::renders_nested_variables_and_detects_cycles` |
| Environments | Secret values kept out of shared files | Done (engine; UI evidence pending) | `C/storage.rs::keeps_secrets_out_of_shared_files` |
| Environments | Clear errors for undefined or unclosed variables | Done (engine; UI evidence pending) | `C/template.rs::reports_undefined_and_unclosed_with_spans`, `C/exec.rs::reports_template_errors_with_field` |

## Template tags and dynamic values

Source: <https://docs.insomnia.rest/insomnia/template-tags>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Template tags | Built-in tags (uuid, now, base64, hash, ...) | Partial: a built-in subset | `C/template.rs::evaluates_builtin_tags`, `C/template.rs::formats_dates`; the full Insomnia tag list is not mapped yet |
| Template tags | Response chaining (value from another request's response) | Done (engine; UI evidence pending) | `S::chains_responses_into_later_requests`, `C/history.rs::chains_values_into_templates` |
| Template tags | Syntax highlighting of tags in fields | Done (engine; UI evidence pending) | `C/template.rs::tokenizes_for_highlighting` |
| Template tags | Custom tags from plugins | Missing | [#21](https://github.com/archipel-dev/nexus-http/issues/21) |

## Authentication

Source: <https://docs.insomnia.rest/insomnia/authentication>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Auth | Basic | Done (engine; UI evidence pending) | `C/auth.rs::basic_header_encodes_credentials`, `S::answers_digest_challenge_and_applies_basic` |
| Auth | Bearer, API key (header/query) | Done (engine; UI evidence pending) | `C/exec.rs::applies_api_key_locations` |
| Auth | Digest | Done (engine; UI evidence pending) | `C/auth.rs::digest_matches_rfc2617_example`, `S::answers_digest_challenge_and_applies_basic` |
| Auth | OAuth 2.0 (client credentials, auth code with PKCE) | Done (engine; UI evidence pending) | `S::fetches_oauth2_client_credentials_token`, `C/auth.rs::pkce_matches_rfc7636_example`, `C/auth.rs::builds_authorization_url_with_pkce`, `C/auth.rs::captures_loopback_redirect` |
| Auth | AWS Signature v4 | Done (engine; UI evidence pending) | `C/auth.rs::sigv4_matches_aws_get_vanilla_vector`, `S::signs_aws_requests` |
| Auth | OAuth 1.0, NTLM, Hawk, Netrc, Akamai EdgeGrid, Atlassian ASAP | Missing | [#25](https://github.com/archipel-dev/nexus-http/issues/25) |

## Cookies

Source: <https://docs.insomnia.rest/insomnia/cookie-management>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Cookies | Cookie jar: store, send, scope, expiry | Done (engine; UI evidence pending) | `S::stores_and_sends_cookies`, `C/cookies.rs::host_only_and_domain_cookies`, `C/cookies.rs::path_secure_and_expiry`, `C/cookies.rs::replaces_same_cookie`, `C/storage.rs::persists_cookies_and_history` |
| Cookies | Cookie manager (view/edit/delete) | Partial: the response pane lists cookies; no manager | [#26](https://github.com/archipel-dev/nexus-http/issues/26) |

## GraphQL, WebSocket, SSE, gRPC

Source: <https://docs.insomnia.rest/> (GraphQL, WebSocket and SSE page URLs to be confirmed);
gRPC: <https://docs.insomnia.rest/insomnia/grpc>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| GraphQL | Queries, variables, schema introspection, schema-aware completion | Done (engine; UI evidence pending) | `S::introspects_graphql_schema`, `C/graphql.rs::parses_introspection`, `C/graphql.rs::resolves_types_and_completes_fields` |
| WebSocket | Connect, send and receive messages | Done (engine; UI evidence pending) | `S::talks_websocket` |
| SSE | Server-Sent Events stream | Done (engine; UI evidence pending) | `S::streams_server_sent_events`, `C/stream.rs::parses_event_stream_per_spec` |
| gRPC | `.proto` and reflection schema, unary and streaming calls | Partial: engine has unit tests, UI and end-to-end evidence pending; unfinished work on an archived branch | `C/grpc.rs::unary_call_sends_metadata_and_decodes_the_reply`, `C/grpc.rs::server_streaming_delivers_every_message`, `C/grpc.rs::client_streaming_sends_initial_and_interactive_messages`, `C/grpc.rs::schema_can_be_fetched_by_reflection`; [#17](https://github.com/archipel-dev/nexus-http/issues/17) |

## Import and export

Source: <https://docs.insomnia.rest/insomnia/import-export-data>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Import | curl command | Done (engine; UI evidence pending) | `C/import/curl.rs::parses_browser_style_command`, `C/import/curl.rs::parses_forms_auth_and_flags` |
| Import | Postman collection v2.1 | Done (engine; UI evidence pending) | `C/import/postman.rs::imports_collection` (v1 rejected: `C/import/postman.rs::rejects_v1`) |
| Import | Insomnia v4 JSON and v5 YAML | Done (engine; UI evidence pending) | `C/import/insomnia.rs::imports_v4_export`, `C/import/insomnia.rs::imports_v5_yaml` |
| Import | OpenAPI 3 | Done (engine; UI evidence pending) | `C/import/openapi.rs::imports_petstore`, `C/import/openapi.rs::survives_cyclic_refs` |
| Import | OpenAPI 2 (Swagger) | Missing: rejected on import (`C/import/mod.rs::rejects_unknown_and_swagger`) | [#23](https://github.com/archipel-dev/nexus-http/issues/23) |
| Import | `.http` / `.rest` files | Done (engine; UI evidence pending) | `C/import/http_file.rs::parses_requests_headers_and_bodies`, `C/import/http_file.rs::bare_urls_default_to_get_and_garbage_is_an_error` |
| Export | Own JSON format, secrets blanked | Done (engine; UI evidence pending) | `C/import/mod.rs::export_round_trips_and_blanks_secrets` |
| Export | Export to `.http`, Postman, OpenAPI, HAR | Missing | [#24](https://github.com/archipel-dev/nexus-http/issues/24) |

## Code generation

Source: <https://docs.insomnia.rest/> (page URL to be confirmed)

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Code generation | Request to code snippets | Partial: 7 targets (curl, Python requests, JS fetch, Go, Rust reqwest, Java HttpClient, Kotlin OkHttp); engine tested, language coverage not compared with Insomnia | `C/codegen.rs::every_target_includes_method_url_headers_and_body`, `C/codegen.rs::escapes_per_language`, `C/codegen.rs::curl_quotes_for_shell`; screenshot pending |

## History, runner, tests, filtering

Sources: collection runner <https://docs.insomnia.rest/insomnia/collection-runner>; tests
<https://docs.insomnia.rest/insomnia/unit-testing>

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| History | Response history per request | Done (engine; UI evidence pending) | `C/history.rs::keeps_bounded_history_newest_last`, `C/storage.rs::persists_cookies_and_history` |
| Collection runner | Run a collection or folder in order, chaining, bail on failure | Partial: engine tested; runner UI lacks iterations, data files and result export | `S::runs_collections_with_chaining_assertions_and_bail`, `C/runner.rs::plans_in_tree_order_and_scopes_to_folders`; [#18](https://github.com/archipel-dev/nexus-http/issues/18) |
| Tests | Response assertions | Partial: declarative assertions only, no scripting | `C/assertions.rs::checks_common_assertions`, `C/assertions.rs::skips_disabled`; scripting/hooks: [#21](https://github.com/archipel-dev/nexus-http/issues/21) |
| Response filtering | JSONPath | Done (engine; UI evidence pending) | `C/filter.rs::selects_scalars_and_lists`, `C/filter.rs::filters_pretty_and_reports_errors` |
| Response filtering | XPath | Missing | [#31](https://github.com/archipel-dev/nexus-http/issues/31) |

## Plugins, sync, design, mocks, CLI

| Area | Insomnia feature | Nexus status | Evidence |
|---|---|---|---|
| Plugins | Extension plugins (<https://docs.insomnia.rest/insomnia/plugins>) | Missing | [#21](https://github.com/archipel-dev/nexus-http/issues/21) |
| Sync / collaboration (git) | Git Sync (<https://docs.insomnia.rest/insomnia/git-sync>) | Partial: storage is git-friendly with secrets kept local, but no git UI | `C/storage.rs::round_trips_workspace_preserving_order`, `C/storage.rs::keeps_secrets_out_of_shared_files`; [#22](https://github.com/archipel-dev/nexus-http/issues/22) |
| Design / OpenAPI editing | Design documents (<https://docs.insomnia.rest/insomnia/design-documents>) | Missing: OpenAPI is import-only | [#23](https://github.com/archipel-dev/nexus-http/issues/23) |
| Mock servers | Mock servers (<https://docs.insomnia.rest/insomnia/mock-servers>) | Missing | [#20](https://github.com/archipel-dev/nexus-http/issues/20) |
| CLI | Inso CLI (<https://docs.insomnia.rest/inso-cli/introduction>) | Missing | [#19](https://github.com/archipel-dev/nexus-http/issues/19) |

## Beyond

Things Insomnia does not offer (or does not offer the same way). "Planned" means not shipped; no
claim is made without a link.

| Area | Capability | Status | Issue / evidence |
|---|---|---|---|
| Files as truth | `.http` files in the repo with readable git diff and merge | Planned. Today only the git-friendly `.nexus/` JSON storage exists | [#11](https://github.com/archipel-dev/nexus-http/issues/11); `C/storage.rs::round_trips_workspace_preserving_order` |
| Speed | Native budgets in CI: cold start < 400 ms, interaction < 16 ms, idle CPU < 1%, memory | Planned | [#13](https://github.com/archipel-dev/nexus-http/issues/13), [#14](https://github.com/archipel-dev/nexus-http/issues/14), [#15](https://github.com/archipel-dev/nexus-http/issues/15), [#16](https://github.com/archipel-dev/nexus-http/issues/16) |
| One suite | Same code standalone and as an Archipel IDE plugin | Planned | [#8](https://github.com/archipel-dev/nexus-http/issues/8), [#9](https://github.com/archipel-dev/nexus-http/issues/9) |
| IDE | Run an HTTP call from a gutter icon next to the route that serves it | Planned | [#27](https://github.com/archipel-dev/nexus-http/issues/27) |
| Codegen | Typed clients and models from responses and collections | Planned | [#28](https://github.com/archipel-dev/nexus-http/issues/28) |
| AI | Optional, local models, never a hidden network call | Planned | [#29](https://github.com/archipel-dev/nexus-http/issues/29) |
| No account | No sign-in, offline, no cloud lock-in; guard test for hidden network calls | Planned (test pending) | [#12](https://github.com/archipel-dev/nexus-http/issues/12) |
| Consistency | One design system with kit conformance and audit tests, mouse-only smoke tests | Planned | [#4](https://github.com/archipel-dev/nexus-http/issues/4), [#5](https://github.com/archipel-dev/nexus-http/issues/5), [#6](https://github.com/archipel-dev/nexus-http/issues/6) |
