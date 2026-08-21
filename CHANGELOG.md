# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.0](https://github.com/r-firth/mux/compare/v0.8.2...v0.9.0) (2026-08-21)


### Features

* add ACP authentication flow ([80c3406](https://github.com/r-firth/mux/commit/80c3406319cea9445582f4092063658ec5b8ffbf))
* add configurable ACP integrations ([c938ccc](https://github.com/r-firth/mux/commit/c938ccce6b4456eedff0f97210d243dcd6163d15))
* add native terminal hyperlinks ([253f3f9](https://github.com/r-firth/mux/commit/253f3f9837b30258f4761f8ed3b51fc56b4444b2))
* adopt Bezel and refine terminal interaction ([8e006de](https://github.com/r-firth/mux/commit/8e006de38d6cc33c572107c3b15713b1477ffcae))
* adopt Ghostty selection gestures ([fdc5fcf](https://github.com/r-firth/mux/commit/fdc5fcfd069414de31f1f6fcfe02ce4b238f1c85))
* build persistent native terminal with ACP agents ([235e1a3](https://github.com/r-firth/mux/commit/235e1a3355495a2c595358837fcaf8f1603c9b7f))
* complete native session lifecycle ([6a5f98a](https://github.com/r-firth/mux/commit/6a5f98ac602d3436c2d061ac70226818267ff5fb))
* expose Zellij resize behind pane mode ([c5e4514](https://github.com/r-firth/mux/commit/c5e45140a3b774a9f888bec79220ff627fff7516))
* make ACP agents first-class panes ([b9d91cd](https://github.com/r-firth/mux/commit/b9d91cd02d95e22282d283c38b70f908cf24e9c0))
* make ACP agents keyboard-first ([ec8cc37](https://github.com/r-firth/mux/commit/ec8cc37b9cba3519fe0c396b56f90eaba2660ff8))
* make agent pane keyboard first ([be28184](https://github.com/r-firth/mux/commit/be28184bf7e955a49ba75435de0971cc160acc62))
* make session switching feel durable ([063bb48](https://github.com/r-firth/mux/commit/063bb48bc085ac914412cb4d89d7df806cada9eb))
* migrate native UI to GPUI ([f1e9c9e](https://github.com/r-firth/mux/commit/f1e9c9e6169661802cdd9c306a55c251023b1b50))
* polish ACP agent timeline ([6bfaba0](https://github.com/r-firth/mux/commit/6bfaba05ee303c6984f59cc2e1a26051fa729553))
* polish native tab renaming ([6e4651b](https://github.com/r-firth/mux/commit/6e4651b6e686d74d7ab16d0ce456b8ca65d1a0b5))
* polish terminal performance and app identity ([3165cde](https://github.com/r-firth/mux/commit/3165cdedf2bb236ab78120a34b909fc719450516))
* scope ACP agents to tabs ([224c40a](https://github.com/r-firth/mux/commit/224c40a62c3de727f5e0ffd2b907ce94790426d7))
* surface ACP slash commands ([d44f303](https://github.com/r-firth/mux/commit/d44f30362ec5cfd22651a09acef45664f116d5c9))
* surface agent attention in tabs ([76de753](https://github.com/r-firth/mux/commit/76de75329dd7c7ac9f8e3845248998684c6e787f))
* turn agents into durable workspaces ([c225e89](https://github.com/r-firth/mux/commit/c225e8966c2b009e0c39b26fabb0dcbdbf750379))


### Bug Fixes

* attach safely to legacy workspaces ([16b59dc](https://github.com/r-firth/mux/commit/16b59dcf26185575b604a326e1bcae73e3570ba3))
* complete release PR lifecycle ([60a4659](https://github.com/r-firth/mux/commit/60a46599e4cec787007ef346c7354055d76245c6))
* enable native IME composition ([045f19c](https://github.com/r-firth/mux/commit/045f19c1d60ebfeac57d66137d5aa8890303d174))
* harden tab and terminal synchronization ([9db3165](https://github.com/r-firth/mux/commit/9db3165268ed506b905b281a75be20ec02a62a7b))
* honor Ghostty cursor blinking ([7ea2e3f](https://github.com/r-firth/mux/commit/7ea2e3ff13dd00365a01474a614ed88075e3e283))
* honor Ghostty font settings ([c6ec19c](https://github.com/r-firth/mux/commit/c6ec19cdde4650de208781c35a54683c5f392d27))
* honor macOS package target architecture ([70eb7b9](https://github.com/r-firth/mux/commit/70eb7b9cd4bca867f64e625b8457ca5908322eaa))
* isolate preview workspaces ([870f8e5](https://github.com/r-firth/mux/commit/870f8e5ad3b5fd3832aed717058f6e849b8da457))
* keep terminal grids aligned ([f2d9acf](https://github.com/r-firth/mux/commit/f2d9acfa355d5d8e6eca81d6daa02e7ed9488630))
* make local macOS signing distribution-safe ([77fccac](https://github.com/r-firth/mux/commit/77fccac72bf8c89f03a0e53dd2bd3c3590e200b1))
* make terminal output sequencing authoritative ([a4eb965](https://github.com/r-firth/mux/commit/a4eb965e938a4a79f7a42e153fd59d5af4210bc6))
* preserve terminal interaction across workspace updates ([a0b67b6](https://github.com/r-firth/mux/commit/a0b67b66b7de755d18106e11a76663f6f98602bd))
* recover from missing ACP runtimes ([3130a77](https://github.com/r-firth/mux/commit/3130a77bd7a502c20d9ef98c1b98d7adaebcb331))
* recover orphaned terminal keyboard state ([dc621c9](https://github.com/r-firth/mux/commit/dc621c96663e3905e48d3c979ae54d5dc8769a95))
* remove ended agents from session picker ([99555a3](https://github.com/r-firth/mux/commit/99555a325c4f1772bc73506626d7418ed1f8504f))
* remove unsupported prose claim ([b14ef10](https://github.com/r-firth/mux/commit/b14ef1013ddb6e10978c79f6463583623a55ed7d))
* report the current terminal version ([c4174f5](https://github.com/r-firth/mux/commit/c4174f56f8b056086f1d2f3d539c13b3ecb13bb0))
* resolve ACP runtimes across daemon boundary ([75ca22f](https://github.com/r-firth/mux/commit/75ca22f30fc55cab09034418687b7bb61e2ee9ce))
* restore native app shortcuts and agent help ([3153140](https://github.com/r-firth/mux/commit/31531408b30a71e49bbf53afdc1a32c7d0d89373))
* restore terminal Tab and Caps Lock input ([4cb0251](https://github.com/r-firth/mux/commit/4cb025124ab232c7f20c5e4443c425ef0f56f746))


### Performance Improvements

* coalesce agent surface updates ([b5df79b](https://github.com/r-firth/mux/commit/b5df79b374430ec7006f0a70636244be1d110387))
* coalesce terminal output and tighten internals ([a013b6d](https://github.com/r-firth/mux/commit/a013b6d5d7820f85716388fde6ea8265ee513818))
* reuse terminal render storage ([defc8f5](https://github.com/r-firth/mux/commit/defc8f5f25cc5f23a1fb5ef85b0309d94ae2857f))


### Reverts

* remove legacy workspace fallback ([fa2be8a](https://github.com/r-firth/mux/commit/fa2be8ae7371dbe5e983a17b4edfed83f9da8fe3))

## [0.8.2](https://github.com/r-firth/mux/compare/v0.8.1...v0.8.2) (2026-08-21)


### Bug Fixes

* remove unsupported prose claim ([b14ef10](https://github.com/r-firth/mux/commit/b14ef1013ddb6e10978c79f6463583623a55ed7d))

## [0.8.1](https://github.com/r-firth/mux/compare/v0.8.0...v0.8.1) (2026-08-21)


### Bug Fixes

* remove ended agents from session picker ([99555a3](https://github.com/r-firth/mux/commit/99555a325c4f1772bc73506626d7418ed1f8504f))

## [0.8.0](https://github.com/r-firth/mux/compare/v0.7.0...v0.8.0) (2026-08-19)


### Features

* make session switching feel durable ([063bb48](https://github.com/r-firth/mux/commit/063bb48bc085ac914412cb4d89d7df806cada9eb))
* surface agent attention in tabs ([76de753](https://github.com/r-firth/mux/commit/76de75329dd7c7ac9f8e3845248998684c6e787f))
* turn agents into durable workspaces ([c225e89](https://github.com/r-firth/mux/commit/c225e8966c2b009e0c39b26fabb0dcbdbf750379))


### Bug Fixes

* recover orphaned terminal keyboard state ([dc621c9](https://github.com/r-firth/mux/commit/dc621c96663e3905e48d3c979ae54d5dc8769a95))

## [0.7.0](https://github.com/r-firth/mux/compare/v0.6.0...v0.7.0) (2026-08-19)


### Features

* adopt Bezel and refine terminal interaction ([8e006de](https://github.com/r-firth/mux/commit/8e006de38d6cc33c572107c3b15713b1477ffcae))


### Bug Fixes

* harden tab and terminal synchronization ([9db3165](https://github.com/r-firth/mux/commit/9db3165268ed506b905b281a75be20ec02a62a7b))
* make local macOS signing distribution-safe ([77fccac](https://github.com/r-firth/mux/commit/77fccac72bf8c89f03a0e53dd2bd3c3590e200b1))
* make terminal output sequencing authoritative ([a4eb965](https://github.com/r-firth/mux/commit/a4eb965e938a4a79f7a42e153fd59d5af4210bc6))
* preserve terminal interaction across workspace updates ([a0b67b6](https://github.com/r-firth/mux/commit/a0b67b66b7de755d18106e11a76663f6f98602bd))

## [0.6.0](https://github.com/r-firth/mux/compare/v0.5.2...v0.6.0) (2026-08-14)

### Features

* add keyboard-first slash command discovery and argument completion
* add project-aware `@` file references as distinct ACP context
* support Zed-compatible custom ACP agent configuration
* make multiple tab-local agent sessions discoverable and pane-navigable

### Bug Fixes

* insert composer newlines with Shift-Enter without submitting
* keep streaming agent conversations pinned to the latest content
* preserve terminal focus when navigating across agent and terminal panes

### Performance Improvements

* keep file indexing off the UI thread and avoid cloning full conversations while rendering

## [0.5.2](https://github.com/r-firth/mux/compare/v0.5.1...v0.5.2) (2026-08-14)

### Bug Fixes

* send Tab and Shift-Tab directly to the focused terminal instead of GUI focus traversal
* honor macOS Caps Lock when encoding terminal input
* prevent terminal Tab from priming an inactive application tab for accidental activation

## [0.5.1](https://github.com/r-firth/mux/compare/v0.5.0...v0.5.1) (2026-08-14)

### Performance Improvements

* coalesce adjacent PTY output before publishing daemon events

### Maintenance

* simplify backend connection state and remove unused dependencies and packaging
* refresh the architecture documentation and README demo

## [0.5.0](https://github.com/r-firth/mux/compare/v0.4.0...v0.5.0) (2026-08-13)

### Features

* add a cohesive Mux app icon, project logo, and concise demo-led README
* add subtle, reduced-motion-aware transitions for pane focus, modes, and agent activity

### Bug Fixes

* preserve native macOS window behavior for Rectangle and other window managers
* keep hidden terminals and static agent indicators from continuously redrawing the app

### Performance Improvements

* send terminal input without a daemon acknowledgement round trip
* remove fixed output latency and batch daemon events into coherent render updates
* cache shaped terminal runs and preserve high-resolution trackpad scroll motion
* publish render frames only for visible terminal panes

## [0.4.0](https://github.com/r-firth/mux/compare/v0.3.0...v0.4.0) (2026-08-13)

### Features

* replace the agent side sheet with keyboard-first agent panes in the terminal grid
* scope agent panes and sessions to their tab, with context from the tab's other terminal panes
* render a responsive ACP timeline with expandable thinking and tool details

### Bug Fixes

* keep streaming conversations pinned to the latest message while preserving intentional scrollback
* wrap agent, tool, and composer content within narrow panes
* navigate out of agent panes in every direction with Option-arrow, including tab fall-through at horizontal edges

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
