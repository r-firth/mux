# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.1](https://github.com/r-firth/mux/compare/v0.9.0...v0.9.1) (2026-10-07)


### Bug Fixes

* a screen drawn in one go is shown whole ([11b76d9](https://github.com/r-firth/mux/commit/11b76d986e656e13fc4e2eadd12e28619c60e8db))


### Performance Improvements

* pane output is parsed on a thread of its own ([51a0a23](https://github.com/r-firth/mux/commit/51a0a238e9a67c695cc78a4ebc3cd92d0ea7432c))

## [0.9.0](https://github.com/r-firth/mux/compare/v0.8.2...v0.9.0) (2026-10-07)


### Features

* a change of tab sends a wake of grain across the window ([b01f0df](https://github.com/r-firth/mux/commit/b01f0df48156ec19c141ba59949cb08db6f6706a))
* a living ground that shows focus, work and calls ([aba3c5f](https://github.com/r-firth/mux/commit/aba3c5f32ea4607ed76d299475ac3bbce1b9bbdc))
* a pane says when its shell has exited ([a4467cd](https://github.com/r-firth/mux/commit/a4467cd4fa21bd49fbdf68bf7b3a07e4dc9eeef5))
* a pane's size shows while it changes, then goes ([e0007b1](https://github.com/r-firth/mux/commit/e0007b1e179a46e62985a3b9882ae68da271fa5b))
* a pane's title sits on its edge, not in a row of its own ([4e9dcf4](https://github.com/r-firth/mux/commit/4e9dcf400136fc327e739b223c4df49c8f4d8bb3))
* a strip that holds twenty tabs ([e34c3a6](https://github.com/r-firth/mux/commit/e34c3a659a8dd9778083daca4bb922024ae202c5))
* a tide keeps the resting ground moving ([d302365](https://github.com/r-firth/mux/commit/d3023657b38d302dcc8c88e60cfa51dc0341da04))
* answer slash commands in the composer, not in toasts ([99ee5c8](https://github.com/r-firth/mux/commit/99ee5c8749d189696567046caec1881265523cd8))
* calmer tab strip, roomier slabs, gofer grain ([5abdd64](https://github.com/r-firth/mux/commit/5abdd645c976cf34b728a9ab994699b5c138088b))
* cmd-click opens links in a pane's output ([21b3ec4](https://github.com/r-firth/mux/commit/21b3ec4e7ae060aa510a6ad30fe287831c927b90))
* cmd-f finds text in a pane, history included ([f501d97](https://github.com/r-firth/mux/commit/f501d9765517a8b9e21f46496f7aae891b3f1231))
* cmd-p goes to any tab, pane, session or command ([73e239c](https://github.com/r-firth/mux/commit/73e239c8898d381379519bfedc6e21e0e4ebe6b2))
* drag the gap between panes to resize them ([8090913](https://github.com/r-firth/mux/commit/8090913e586a625597ec1e52f8db95d44faef4f5))
* errors say themselves in the strip, tooltips in mono ([f793d14](https://github.com/r-firth/mux/commit/f793d14baabdaeb07f54fa181d73badfcf1d6885))
* install a missing agent from the launcher ([588639d](https://github.com/r-firth/mux/commit/588639dd79699a1b7bdb021aa839c454a55f38dd))
* no two of the ground's dots are alike ([acb1181](https://github.com/r-firth/mux/commit/acb11817fb344884922dc537ad5064c183fdee54))
* panes keep Ghostty's history, even across tab switches ([610665f](https://github.com/r-firth/mux/commit/610665f0908c45f8efd6d1d95951915683909983))
* panes out of sight say when they want you ([63aa900](https://github.com/r-firth/mux/commit/63aa9005f7cb25eae1506e5b59b45c15fdff1fb5))
* quick select puts a key on every URL, path and hash on screen ([89bc221](https://github.com/r-firth/mux/commit/89bc2215655e755cc7a5852e8fc152d48bfd2c26))
* rebuild the agent pane in gofer's style ([be5ce5e](https://github.com/r-firth/mux/commit/be5ce5e07476e5deda7ed60029af314d5f79d5ba))
* rename a tab in its own chip ([af99ae5](https://github.com/r-firth/mux/commit/af99ae51317528771900d7f67dba18084968fc54))
* sessions in a sheet under the session's name ([4ad52cf](https://github.com/r-firth/mux/commit/4ad52cfc26f31163cedf23b18648d4b4d0096214))
* settings as a slab of agents, read fresh each time ([86d1282](https://github.com/r-firth/mux/commit/86d12820ff62133b1803b27d6ecc0f1b8bc6bbd4))
* slabs glide into place, and focus glows across ([f430769](https://github.com/r-firth/mux/commit/f43076937cfacf204352da3a37e32f63641b0728))
* steadier keys and clearer steps in the agent pane ([ece460c](https://github.com/r-firth/mux/commit/ece460c8e07601c96bbdb7793420ec3233bc6da8))
* the grain moves on every frame, in a view of its own ([2d670f2](https://github.com/r-firth/mux/commit/2d670f2f0d03bcf60be7a29119c170fe191cff08))
* the ground is a panel of round dots ([02d3243](https://github.com/r-firth/mux/commit/02d3243ca378e9164d25c05db325d5a154aa7acb))
* the ground is drawn by the GPU ([04fad24](https://github.com/r-firth/mux/commit/04fad24e1e00b7ecbe3e4d6bfb0997badeec260e))
* the ground is the same dots wherever it is drawn ([a716844](https://github.com/r-firth/mux/commit/a716844d5df4dc99fecdfa6fcfef4dfdabbe1d30))
* the strip says what a copy took ([e29d2e5](https://github.com/r-firth/mux/commit/e29d2e58075e0219c86b19f3dc2273dec39c4e91))
* typing returns a scrolled pane to its latest output ([516373e](https://github.com/r-firth/mux/commit/516373e686143076a86d3b8de5255dc575a3c1d9))
* warm slab chrome with per-tab inks ([4748fa2](https://github.com/r-firth/mux/commit/4748fa23380048a6173f0e76778f3437f6624da4))


### Bug Fixes

* a drag whose release was missed no longer holds the pointer ([b895809](https://github.com/r-firth/mux/commit/b8958095bf4056698289184733aa9c12eb006722))
* a tab's only pane wears its legend too ([1324b4c](https://github.com/r-firth/mux/commit/1324b4c0a533043b6197ba0b5dddd5654ec2e0ff))
* bring back the dithered grounds ([e0ad6e7](https://github.com/r-firth/mux/commit/e0ad6e734f8fb4c14310be52b5a2160adb334c2f))
* go to sits beside the session name, apart from ctrl-p ([a4cfdb7](https://github.com/r-firth/mux/commit/a4cfdb74b4968ea3c1bbf68d7513c1bb5b06c446))
* keep a macOS-only field out of other builds ([314ecc5](https://github.com/r-firth/mux/commit/314ecc5359bbb0133940451fa8d8af785aa7d5b5))
* the agent pane's head no longer spells out the folder name ([6603c41](https://github.com/r-firth/mux/commit/6603c41f7ecc8c76b59fc74732aba38e3703bdfd))
* the first tab keeps clear of the traffic lights ([6c68658](https://github.com/r-firth/mux/commit/6c68658c765cb3b937df6caa5b7dc747257f2bdf))
* the wake runs through the grain, never over a pane ([4f340e3](https://github.com/r-firth/mux/commit/4f340e36da0797cfe9c2d976255112b018931b5c))


### Performance Improvements

* a frame rewrites only the rows that changed ([9b98f89](https://github.com/r-firth/mux/commit/9b98f898ddc12a41a77ab2c9c0d2acbfa1ff36d0))
* a pane that didn't change is not drawn again ([50658a6](https://github.com/r-firth/mux/commit/50658a64584921e5ce9b215daac70d8b8502d5dd))
* a typed key's echo is sent without waiting ([0b561a5](https://github.com/r-firth/mux/commit/0b561a57e75a5daec1cb5a01c3799bbac74ef6e3))
* backgrounds are painted as runs, not cell by cell ([7e2226d](https://github.com/r-firth/mux/commit/7e2226da4e8944e0f9d76e9fcfb2895938c91a37))
* heavy output no longer freezes the window ([35a04ef](https://github.com/r-firth/mux/commit/35a04ef80e77cf02f05b6ed76f85901f2b8a03b7))
* read pane output as byte runs ([85238cf](https://github.com/r-firth/mux/commit/85238cf248d8ceb15bfc28bf450d1488ece0f505))
* skip the dots under panes ([1a3dd5f](https://github.com/r-firth/mux/commit/1a3dd5f09e8cda290e98a3b39c7549bc4de69744))
* the ground no longer stalls the app while a pane prints ([baf0c47](https://github.com/r-firth/mux/commit/baf0c47ecedcd6dde7633a5115d211d8bc93fa98))


### Reverts

* the ground is the same dots wherever it is drawn ([ff7c658](https://github.com/r-firth/mux/commit/ff7c658d0cab61805f5fc2ec78c675d528676a08))

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
