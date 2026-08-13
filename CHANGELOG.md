# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0](https://github.com/r-firth/mux/compare/v0.2.0...v0.3.0) (2026-08-13)


### Features

* add configurable ACP integrations ([4206103](https://github.com/r-firth/mux/commit/4206103d049ddb0157664cf92ce1a8d1ce426320))
* expose Zellij resize behind pane mode ([ff448ee](https://github.com/r-firth/mux/commit/ff448ee41c127f056179796aae09df753d0ab541))
* make agent pane keyboard first ([2de7f1d](https://github.com/r-firth/mux/commit/2de7f1d511482550798d1dcb299874341b3ccb08))
* migrate native UI to GPUI ([41aeea7](https://github.com/r-firth/mux/commit/41aeea7ed688f90d48a38343d7ecfc619c080485))
* polish ACP agent timeline ([bf7f827](https://github.com/r-firth/mux/commit/bf7f8279abf06745bab7052d9246d434179d9e08))
* scope ACP agents to tabs ([196e5c7](https://github.com/r-firth/mux/commit/196e5c76658aef3aa34b0d0520a1055c9692a428))


### Bug Fixes

* attach safely to legacy workspaces ([2a202f3](https://github.com/r-firth/mux/commit/2a202f3e25bf9686cd3d66ecb4063336990fff2d))
* support cross-architecture macOS packaging
* complete release PR lifecycle ([f8062a3](https://github.com/r-firth/mux/commit/f8062a3c5fe8ea40fbce8cc6542935941b8e9fc9))
* honor Ghostty font settings ([42a0be0](https://github.com/r-firth/mux/commit/42a0be0f89ab7b9bafe68212ba4652e9c76adc49))
* isolate preview workspaces ([780c8f5](https://github.com/r-firth/mux/commit/780c8f555ced39178c3d988bd1a1f1285b6fa4fd))
* keep terminal grids aligned ([83ad1c0](https://github.com/r-firth/mux/commit/83ad1c02b87e22df6fe5b592b5080afe739d2959))
* recover from missing ACP runtimes ([76e6162](https://github.com/r-firth/mux/commit/76e6162bdfd10425f77921f7ac1bf3ac7734dacb))
* resolve ACP runtimes across daemon boundary ([30132ef](https://github.com/r-firth/mux/commit/30132ef537e36ef70fd94d3ee2e775e9ce8ec215))
* restore native app shortcuts and agent help ([3d7b96f](https://github.com/r-firth/mux/commit/3d7b96f1196ddcb620b5fe04325c7c62bd9b01e8))


### Reverts

* remove legacy workspace fallback ([4e5c81e](https://github.com/r-firth/mux/commit/4e5c81e6054ffe68d7bf4a2b0a035846cd7d7ad3))

## [0.2.0](https://github.com/r-firth/mux/compare/v0.1.0...v0.2.0) (2026-08-13)


### Features

* add ACP authentication flow ([0027c92](https://github.com/r-firth/mux/commit/0027c92317294c161037c053450b3c6975b5e6b1))
* add native terminal hyperlinks ([7360f42](https://github.com/r-firth/mux/commit/7360f4223f4987e9433bb74705fd4f76b4f123b9))
* adopt Ghostty selection gestures ([8b577d0](https://github.com/r-firth/mux/commit/8b577d06325e1833c7854f76c64ed23b3ca60510))
* complete native session lifecycle ([2d9ee99](https://github.com/r-firth/mux/commit/2d9ee99944eef7289aa5fb7d6a80ae5dcaabf474))
* polish native tab renaming ([9459db7](https://github.com/r-firth/mux/commit/9459db7a86d809f0a47bcd5e87dafbdfb67fc552))
* surface ACP slash commands ([14c24bb](https://github.com/r-firth/mux/commit/14c24bb1bce48f41ab6b773f247e801f6aa6acb9))


### Bug Fixes

* enable native IME composition ([2ab0311](https://github.com/r-firth/mux/commit/2ab03110e3ff999f247a504b07506ff70163c325))
* honor Ghostty cursor blinking ([32fdec0](https://github.com/r-firth/mux/commit/32fdec03bb1c233848039a79d36c8510383a7be1))
* report the current terminal version ([0389e09](https://github.com/r-firth/mux/commit/0389e092c053c644215af31e5157534aafd0822b))


### Performance Improvements

* coalesce agent surface updates ([6d3c190](https://github.com/r-firth/mux/commit/6d3c190f858bf7e08b724391315d7b8aa7d7f493))
* reuse terminal render storage ([e65f07e](https://github.com/r-firth/mux/commit/e65f07e7536754d74c8d1fbff3dca22e8fe334fc))

## [Unreleased]

## [0.1.0](https://github.com/r-firth/mux/releases/tag/v0.1.0) - 2026-08-13

### Added

- build persistent native terminal with ACP agents
