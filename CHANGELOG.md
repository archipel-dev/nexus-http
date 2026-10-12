# Changelog

## [0.1.2](https://github.com/archipel-dev/nexus-http/compare/v0.1.1...v0.1.2) (2026-10-12)


### Bug Fixes

* **ui:** bring back the main toolbar in the standalone window ([#51](https://github.com/archipel-dev/nexus-http/issues/51)) ([f265b35](https://github.com/archipel-dev/nexus-http/commit/f265b355047416af3141a201cfc2211ee2f3f65d))

## [0.1.1](https://github.com/archipel-dev/nexus-http/compare/v0.1.0...v0.1.1) (2026-10-12)


### Bug Fixes

* **dist:** one file per MSI component so the Windows installer builds ([#49](https://github.com/archipel-dev/nexus-http/issues/49)) ([97d9308](https://github.com/archipel-dev/nexus-http/commit/97d9308c0229b7a88f3f8c3141378b48c3c67249))

## 0.1.0 (2026-10-12)


### ⚠ BREAKING CHANGES

* **ui:** the hand-painted widget set (widgets.rs, theme.rs) is gone; the embedding API is NexusApp::ui for a host and NexusApp::window for the standalone app.

### Features

* add Archipel plugin adapter on archipel-sdk ([#34](https://github.com/archipel-dev/nexus-http/issues/34)) ([73feb31](https://github.com/archipel-dev/nexus-http/commit/73feb3107a9e945f4611faadcc76063ab29566f0))
* lay out the app with kit SplitPane, EmptyState and Toast ([#43](https://github.com/archipel-dev/nexus-http/issues/43)) ([f9b9eb2](https://github.com/archipel-dev/nexus-http/commit/f9b9eb29f7bbb0e5ad23a6228af9361016d03205))
* move nexus engine, views and standalone app into workspace ([#2](https://github.com/archipel-dev/nexus-http/issues/2)) ([1ac43a9](https://github.com/archipel-dev/nexus-http/commit/1ac43a9423a3179aedf978721b5491e7150f426f))
* port response viewer and runner to archipel-kit ([#37](https://github.com/archipel-dev/nexus-http/issues/37)) ([fa1eaa5](https://github.com/archipel-dev/nexus-http/commit/fa1eaa528e22c198a543400bac7bfbb0c9c17535))
* port sidebar and dialogs to archipel-kit ([#33](https://github.com/archipel-dev/nexus-http/issues/33)) ([6681487](https://github.com/archipel-dev/nexus-http/commit/6681487d2de97e75de5a6db43ed16125b5ce1906))
* **ui:** Islands workbench, built only from archipel-kit ([#46](https://github.com/archipel-dev/nexus-http/issues/46)) ([f45cbfa](https://github.com/archipel-dev/nexus-http/commit/f45cbfa1968eb9eed02501d539a9049b1cc9a6ef))


### Bug Fixes

* handle streaming requests in the collection runner ([#42](https://github.com/archipel-dev/nexus-http/issues/42)) ([07fb791](https://github.com/archipel-dev/nexus-http/commit/07fb791a1b7c9a2c10f00a27406b205c6da884f9))


### Documentation

* add README, roadmap and Insomnia parity table ([#32](https://github.com/archipel-dev/nexus-http/issues/32)) ([4bbac27](https://github.com/archipel-dev/nexus-http/commit/4bbac27fefcb4aa20a0223e5dbd06f0992a12089))
