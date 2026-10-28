# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.10.0](https://github.com/r-firth/mux/compare/v0.9.1...v0.10.0) (2026-10-28)


### Features

* a bell in the pane on screen flashes it ([f4b35b6](https://github.com/r-firth/mux/commit/f4b35b6f7f41e4758346dc6159498a09e00dc383))
* a blinking bar cursor at the shell's prompt, as in Ghostty ([8698e61](https://github.com/r-firth/mux/commit/8698e61faffb7cf21e3281bd55d4d4a866edda03))
* a failed command is marked in the margin beside its prompt ([1b0ba4c](https://github.com/r-firth/mux/commit/1b0ba4c138d20beb8ebf81e672e32efb71e77f39))
* a failure mark says how its command ended and selects its output ([498e10c](https://github.com/r-firth/mux/commit/498e10c8a8ba594afdb030d9db84d084fb24d0e0))
* a long command that fails out of sight says so ([0b228e6](https://github.com/r-firth/mux/commit/0b228e609887f67d8926e2e11c4bbcecd3dacf1e))
* a long program that finishes out of sight says so ([5982f26](https://github.com/r-firth/mux/commit/5982f26371f2d0ff6624e098d2f7ef996d714597))
* a pane's head keeps its directory beside the command a shell titles it with ([8309e22](https://github.com/r-firth/mux/commit/8309e22d76b9aa7bbab1d06b1aeeb3a1850d6117))
* a pane's head says when a password is being typed into it ([e8307fe](https://github.com/r-firth/mux/commit/e8307fe58059db79785705bfad554570527372ee))
* a paste that would run lines asks for a second Cmd+V ([64908b3](https://github.com/r-firth/mux/commit/64908b3f15eb959655cc48e4c552ccb0ef2f2d9e))
* a pill on a scrolled-back pane says how much is below ([1bdb75d](https://github.com/r-firth/mux/commit/1bdb75d68d37384e95d6c7b2dab67dd3117a19cf))
* a plain click in the command being typed moves the cursor there ([b4111fb](https://github.com/r-firth/mux/commit/b4111fb0ce848333e994d84188e1b61e8f87af8e))
* a program that finished cleanly calls in its tab's ink, not peach ([4725f09](https://github.com/r-firth/mux/commit/4725f0920181b5f057e8ff19610535d6003850fc))
* a program's clipboard read is asked about in the strip, Cmd+V giving it, as Ghostty's default clipboard-read = ask ([01627d4](https://github.com/r-firth/mux/commit/01627d43f529e2033eab7ec72187abf2907aac9f))
* a program's notification reaches Notification Center while Mux is in the background, as Ghostty's do ([892dce2](https://github.com/r-firth/mux/commit/892dce24f03a8df2f6c7cc22c68cac3bb18904a9))
* a prompt gone to washes for a moment in the tab's ink ([b368c98](https://github.com/r-firth/mux/commit/b368c9885568c834a13eacb045f077f725dd6e50))
* a quiet pane's history is compressed, in the daemon and the window, and comes back compressed on reattach ([9f6c6ca](https://github.com/r-firth/mux/commit/9f6c6caf9f53a81d35ed2d2b7bd042eab3adc44d))
* a reopened tab's panes show what they showed ([88952db](https://github.com/r-firth/mux/commit/88952db9c06367afc0d403b719c78f1321368d3b))
* a right click on a pane opens a menu of what can be done to it ([bc8afe6](https://github.com/r-firth/mux/commit/bc8afe6b0fb3b486f855979c2c1ec5cb816c42fd))
* a selection dragged past a pane's edge scrolls the history ([db41c02](https://github.com/r-firth/mux/commit/db41c025c560948fae7363a40d7b09a1ca3622f2))
* a tab can be closed from its chip ([62a8b80](https://github.com/r-firth/mux/commit/62a8b80e566bca6efd1c485e407a5ac7117219ae))
* a tab can move along the strip, and a pane can take a tab of its own ([799f844](https://github.com/r-firth/mux/commit/799f8449eb01f966b58e03d8ce57cd137c4d6fbe))
* a tab is named for the program running in it ([29ae904](https://github.com/r-firth/mux/commit/29ae904ab184eaf28473e2ca0389dad4baf4ab3a))
* a tab keeps its colour when it moves and when the app restarts ([df0d8f1](https://github.com/r-firth/mux/commit/df0d8f15601b2b38709e621663b3a1f981db57f6))
* a tab out of sight breathes its dot while a program runs in it ([8bdf459](https://github.com/r-firth/mux/commit/8bdf459982bba52c04f26451414ee3111837a936))
* a tab's tooltip says where it is in full ([2ce79c5](https://github.com/r-firth/mux/commit/2ce79c5c3c245a21f8056ce8daa0898083b117e2))
* a zoomed pane's head says when a pane hidden behind it calls ([12f4211](https://github.com/r-firth/mux/commit/12f4211ff3ed03bc79c1cc76ade6b508b5fd24bf))
* an unfocused split marks its cursor with a hollow block, as Ghostty does ([71cf021](https://github.com/r-firth/mux/commit/71cf02197badc430dac2db4236ee3dd0ce6eb7bd))
* blinking cursor, from programs and Ghostty's cursor-style-blink ([a839dab](https://github.com/r-firth/mux/commit/a839dab321788c11a2e306ba2d7157d1cdd4f9b2))
* clipboard-read = allow from Ghostty's config answers programs' OSC 52 reads ([9380f19](https://github.com/r-firth/mux/commit/9380f19b8f36e776db6109cf4fc0753d4fa8e608))
* closing a pane that is running something asks first ([a34c92c](https://github.com/r-firth/mux/commit/a34c92c034f1a65b0620f732497b9cf3651b7ab6))
* closing follows confirm-close-surface from the Ghostty config ([1e663d5](https://github.com/r-firth/mux/commit/1e663d55f9c48753bde62c303ebbf97583a0a3d9))
* Cmd-click a path in the output to show it ([3992d7c](https://github.com/r-firth/mux/commit/3992d7c51f8e8aaf5487afed924dbe24f8caa2b4))
* Cmd+A selects everything in a pane ([abe4d20](https://github.com/r-firth/mux/commit/abe4d2002022f3eba00862008866d1d763b6233b))
* Cmd+arrows and Cmd+Backspace edit the line ([658fff1](https://github.com/r-firth/mux/commit/658fff1dd4fe923a0d12ebef0aaf16c378b66086))
* Cmd+Control+= evens out a tab's panes ([0050a1d](https://github.com/r-firth/mux/commit/0050a1df4fbc7683d403da09652531b6fddbb08c))
* Cmd+E finds the selection ([22e82c2](https://github.com/r-firth/mux/commit/22e82c230d6b06e2a25509b773ea8284007f22a2))
* Cmd+J also goes to an agent waiting on you in another tab ([ef9ca50](https://github.com/r-firth/mux/commit/ef9ca50f02e2d8fda57cadbcefa494936e1a1c08))
* Cmd+J goes to the pane that called last ([a6f279e](https://github.com/r-firth/mux/commit/a6f279e139d5eacfbc113894794863394f47bf90))
* Cmd+K clears a pane ([6b0c682](https://github.com/r-firth/mux/commit/6b0c682eb1cf684dff5f7a236934bba00850d45f))
* Cmd+M, Cmd+H and Control-Cmd+F do what they do in every Mac app ([8a727d5](https://github.com/r-firth/mux/commit/8a727d5794623e1ef9c3f6a750783c7c9b7cca99))
* Cmd+N starts a session where the focused pane is ([844dfd5](https://github.com/r-firth/mux/commit/844dfd5ed09ad78655c8973cba1b90b7a223b478))
* Cmd+Option+W closes the tab, as in Ghostty ([dd202ee](https://github.com/r-firth/mux/commit/dd202eed4d6af5edddace3f2a356b956487e7118))
* Cmd+P copies the last command's output in one go ([0a13690](https://github.com/r-firth/mux/commit/0a13690e49e06aee88a37b40b8d671cf3d054f12))
* Cmd+P finds commands run in any pane of the session ([9b35145](https://github.com/r-firth/mux/commit/9b3514530cabcf6e2927d15466c11848f8ef3daf))
* Cmd+P names the pane a command ran in when it has a name ([d664573](https://github.com/r-firth/mux/commit/d6645736b41dd2351fdf88326ed5994185eb7852))
* Cmd+Shift+, reloads the Ghostty config ([c640854](https://github.com/r-firth/mux/commit/c6408548815bd1028c54bcc332bd17efb368353f))
* Cmd+Shift+O selects the last command's output ([fcf2a83](https://github.com/r-firth/mux/commit/fcf2a83fd86c01cb414b2c2e930b92f4a5f53c55))
* Cmd+Shift+T brings back a closed pane as well as a closed tab ([d76fb82](https://github.com/r-firth/mux/commit/d76fb821d4c792caf0a91392871050f37290774f))
* Cmd+Shift+T reopens the tab closed last ([2b4d154](https://github.com/r-firth/mux/commit/2b4d15421760a4549048b571b3b43841d284bee9))
* Cmd+Shift+V pastes the last selection, as Ghostty and Terminal.app ([abaa074](https://github.com/r-firth/mux/commit/abaa074ea5f0c8c234685eeb7bda3293e950856b))
* Cmd+Up and Cmd+Down jump between shell prompts ([7040c63](https://github.com/r-firth/mux/commit/7040c63838348d2ebfdd6ae7978d3684138d1826))
* Cmd+Z just after a close brings it back, as Ghostty's undo ([2925dd4](https://github.com/r-firth/mux/commit/2925dd4243cdb38b21ae71791d1b858ba483ff67))
* Cmd+Z's window after a close follows Ghostty's undo-timeout ([2245790](https://github.com/r-firth/mux/commit/22457901507794e70aa7f781a22427b9fe3d8dc6))
* copy-on-select = clipboard puts selections on the clipboard ([dc5acd7](https://github.com/r-firth/mux/commit/dc5acd77afa66607b55bda3b1f78bda8e5decb3b))
* count how long a program has run in its pane's head ([9466564](https://github.com/r-firth/mux/commit/9466564cfac863d93d62f82268124e0fd081742f))
* Ctrl+p c names a pane ([6316472](https://github.com/r-firth/mux/commit/63164729a9048a2f475238ad2c446207e5feb7b6))
* Ctrl+p n splits a pane across its longer side ([1cb9fe0](https://github.com/r-firth/mux/commit/1cb9fe0dd19d318e0d7d0a01efb22781da83cbf5))
* Ctrl+t s types into every pane of a tab ([6e1de6d](https://github.com/r-firth/mux/commit/6e1de6d620eca73e9ffc7428b402f27e20f458a7))
* cursor-color and cursor-text from the user's own Ghostty config ([1ae29dc](https://github.com/r-firth/mux/commit/1ae29dc59ab2b4618e45bf43d48b2c282f468dc6))
* cursor:steady holds the prompt's bar still, as in Ghostty ([64fe612](https://github.com/r-firth/mux/commit/64fe612d316128fc8a54786efaceca1376c60f0a))
* dead keys and input methods compose text in a pane ([4b287d4](https://github.com/r-firth/mux/commit/4b287d46494fcc43ce26d2b49052751755a4b21e))
* desktop-notifications = false from Ghostty's config leaves programs' notifications unheeded ([edfbda1](https://github.com/r-firth/mux/commit/edfbda166625529fd2dff88ffabf00536c9141d0))
* double-click a pane's head to name it ([fb4f02f](https://github.com/r-firth/mux/commit/fb4f02f69ec41877f2eebc3acb865163e96bed98))
* drag a pane's head onto another pane to trade places ([6ae3c91](https://github.com/r-firth/mux/commit/6ae3c91704f1d81b2c92eac8011e31193a6e6f63))
* drag the scroll thumb to move through history ([17ccb23](https://github.com/r-firth/mux/commit/17ccb2326f917f9529d98afaffa0ef75fdd8ab0d))
* drop a pane near another's edge to put it on that side ([4bd8cb5](https://github.com/r-firth/mux/commit/4bd8cb55d94f00b1169aca48592dccd47f88fcc9))
* drop a pane on a tab chip to move it there ([a5c84fd](https://github.com/r-firth/mux/commit/a5c84fdf0849a465e5d6c23186d382c9e456aefc))
* files a Ghostty config includes with config-file are read too ([fe6bb3e](https://github.com/r-firth/mux/commit/fe6bb3e9c5728876f9975cd333913ae3814c1c6c))
* files dropped on a pane type their paths ([9319ec6](https://github.com/r-firth/mux/commit/9319ec60336a5035325f81974de2a7edb0b9e17e))
* find the commands a pane ran from the palette ([9ccd7ae](https://github.com/r-firth/mux/commit/9ccd7aeb65bc30551551604e21048ae9d8113486))
* focus follows the mouse when Ghostty's config says so ([e73648c](https://github.com/r-firth/mux/commit/e73648cac8265e0b93065b72072edefef7c6a068))
* font features from Ghostty's config ([03c6551](https://github.com/r-firth/mux/commit/03c6551d5048bcf10991f61f707d260cc1ecced8))
* Ghostty's clipboard-trim-trailing-spaces = false keeps the spaces at the ends of copied lines ([dab2534](https://github.com/r-firth/mux/commit/dab253471728f8099fd6c0ebb90ee2c2f99787ad))
* Ghostty's copy_url_to_clipboard keybind action copies the link under the pointer ([7af6fd2](https://github.com/r-firth/mux/commit/7af6fd25e58fd7ada488b0d9baac493bce475593))
* Ghostty's cursor-opacity lets the text show through the cursor ([666f478](https://github.com/r-firth/mux/commit/666f4780a2978605f0e9db4c31c4236fc99a7349))
* Ghostty's link-url = false leaves written URLs and paths as text ([c6a5c43](https://github.com/r-firth/mux/commit/c6a5c43e8c584386037646795e72c5f872fd7b8d))
* Ghostty's move_tab and undo keybind actions ([5ee07ba](https://github.com/r-firth/mux/commit/5ee07baffd7db3bc09783c2d01a3434ecbc0e230))
* Ghostty's selection-clear-on-typing = false keeps a selection while you type ([3aa6b40](https://github.com/r-firth/mux/commit/3aa6b40e0560071e7f063355cdb9bba786fb9f58))
* Ghostty's window-new-tab-position = current opens a new tab just after the current one ([f2eb2f0](https://github.com/r-firth/mux/commit/f2eb2f0571ac8dccee948ac49aa8262ff2c4e6be))
* Go to finds text size and the next pane ([6f5fb1e](https://github.com/r-firth/mux/commit/6f5fb1ed158106079782fd5bea64f390f955a5b1))
* holding Cmd a moment numbers the tabs for Cmd+1 through Cmd+9 ([29d6c8e](https://github.com/r-firth/mux/commit/29d6c8e6803901b416cc9382733c6112e88171bb))
* iTerm's keys move between panes and resize them ([ce5ad86](https://github.com/r-firth/mux/commit/ce5ad8672733aef883ba728b3d090392989e87a2))
* keybind lines from Ghostty's config ([d697078](https://github.com/r-firth/mux/commit/d697078700b2e7d0d6b905832e254043d212149f))
* libghostty 1.4, with Kitty images drawn in panes and synchronized output ([16c48af](https://github.com/r-firth/mux/commit/16c48afd9763f89089eb8f8f687e2eb4385cde8f))
* Locked mode, Ctrl+p Ctrl+g, hands every key to the program until Ctrl+g ([9c4a0bb](https://github.com/r-firth/mux/commit/9c4a0bb8589abf0e15a7a51af37ac3882791e9b1))
* mark the commands that failed while scrolled back ([c86a1be](https://github.com/r-firth/mux/commit/c86a1be8e92c45115b1133c49f2d69ea55311cd5))
* minimum-contrast from Ghostty's config lifts faint text ([1fed0c5](https://github.com/r-firth/mux/commit/1fed0c56d5254e787b66e0d04ef882534993d447))
* more of Ghostty's keybind actions: scroll by lines or a share of a page, name the pane, open the history ([ee8182c](https://github.com/r-firth/mux/commit/ee8182c6a32ccc94e0740322273f9d316342c55b))
* mouse-hide-while-typing from Ghostty's config ([81e5fa2](https://github.com/r-firth/mux/commit/81e5fa29c0312acb643759f41cb122efe5287001))
* muxctl can type into a pane, change the workspace and wait on an agent ([d1804ff](https://github.com/r-firth/mux/commit/d1804ff1045377dc4f36af35f7f6475934b573ee))
* notify-on-command-finish from Ghostty's config ([92b431c](https://github.com/r-firth/mux/commit/92b431c5e8e191b4d9b07aa0d58f832cac26b4e4))
* notify-on-command-finish-action = notify posts a finished command to Notification Center ([62bca56](https://github.com/r-firth/mux/commit/62bca56d8c7c0cf24521921a7fbc3ef4cc2c8b02))
* open a pane's history in a text editor ([85fe03e](https://github.com/r-firth/mux/commit/85fe03e8fcbce0c95a143f5841b3ff0caec69d56))
* Option is Meta when Ghostty's config says so ([4c76e29](https://github.com/r-firth/mux/commit/4c76e290556f0617e976f09c5659b9867de9dcb8))
* Option-click moves the cursor to where you click ([56f0e01](https://github.com/r-firth/mux/commit/56f0e01729745de91a3f12c94a26e148087eda97))
* Option+arrows go a word back or on, as Ghostty does on a Mac ([6719aa6](https://github.com/r-firth/mux/commit/6719aa6089b1965e3302e5d66d9079cefdeef033))
* Pane and Tab mode hints name Ctrl+g for Locked mode ([5679ea6](https://github.com/r-firth/mux/commit/5679ea6605da4d088a74004ae398ddcf0596f800))
* panes keep what they showed across a daemon restart ([6199eaa](https://github.com/r-firth/mux/commit/6199eaa32fbbafa035853dc893717fbf8743f916))
* panes say they are in Mux, not the terminal Mux came from ([0820979](https://github.com/r-firth/mux/commit/0820979df5be87052240824177fe4d61936b7a8d))
* panes show the cursor style from the Ghostty config ([272ef30](https://github.com/r-firth/mux/commit/272ef30bfbb5320b66ea5255ab614d6d566ff1e2))
* panes show what a program says it is doing (OSC 7501), and the pointer it asks for (OSC 22) ([c340d28](https://github.com/r-firth/mux/commit/c340d2803bd44bfc1ade3ba08756a7f128a74322))
* pin the command's prompt above its scrolled-back output ([f0d1cda](https://github.com/r-firth/mux/commit/f0d1cda54deb0eb1131e423bb267ea9505b657b8))
* programs can copy to the clipboard with OSC 52 ([1827069](https://github.com/r-firth/mux/commit/1827069985cfe07ed5138d63bb9988ca24ffac2b))
* progress bars from programs that report progress ([e7f1e30](https://github.com/r-firth/mux/commit/e7f1e30531887188365d9183378149b388083189))
* prompt ticks beside the scroll thumb ([84ad2f0](https://github.com/r-firth/mux/commit/84ad2f034a5af9bf6d7fc82a941b12552589e357))
* reset a pane's terminal from its menu, Cmd+P or Ghostty's reset keybind ([6f01ee5](https://github.com/r-firth/mux/commit/6f01ee5c74f7451cd99d7895b4ad8bed41a97eae))
* right-click a command to copy it or its output ([ef1c493](https://github.com/r-firth/mux/commit/ef1c4934973de627f748bb09d7c28c6286bfb14b))
* right-click a command to copy it with its output ([4684ef5](https://github.com/r-firth/mux/commit/4684ef5acdc691e2ad167bbd9efc10d99c7712a5))
* right-click a link to open or copy it ([6201c4f](https://github.com/r-firth/mux/commit/6201c4f728ef63e92ddeecdd2a14a94d256b775f))
* right-click a tab's chip for its menu ([7e24d71](https://github.com/r-firth/mux/commit/7e24d71d75f8fe97a96625c8e7b1e5a0df7180bb))
* right-click the pinned command line for its copy menu ([39dd77c](https://github.com/r-firth/mux/commit/39dd77cc4ac1d1b0f0daf12cfe3e5212b24306b2))
* right-click-action from Ghostty's config ([72d3149](https://github.com/r-firth/mux/commit/72d3149c7545db3fbe2d2dde66ec02cc1b8aea26))
* rows follow adjust-cell-height from the Ghostty config ([b22843f](https://github.com/r-firth/mux/commit/b22843fa4614a9fc2a6c3f252006804e64710e2c))
* say how long each command ran ([c3ef363](https://github.com/r-firth/mux/commit/c3ef3630a9d76a2a3a4e8e6049b738213ae269e9))
* selection colours from Ghostty's config ([c177bbd](https://github.com/r-firth/mux/commit/c177bbd96220a0610bcbf7b7ea11afa14a24f359))
* selections made with the keys are kept for Cmd+Shift+V too ([9b8c7b8](https://github.com/r-firth/mux/commit/9b8c7b8663f1caa974b073dd87c3c0e6f251dba2))
* settings say which Ghostty config the terminal follows ([f0a84b7](https://github.com/r-firth/mux/commit/f0a84b72cb544d0000511e8bcb6a8be0f252fa53))
* settings say which Ghostty keybinds Mux follows ([0647b63](https://github.com/r-firth/mux/commit/0647b638e6b0987da574bc87f06293bfc27d79ce))
* Shift with an arrow moves the end of a selection ([a64cd42](https://github.com/r-firth/mux/commit/a64cd4213fa44928f8b2d5dd8269499dcebc3c52))
* Shift-click extends a selection ([5ffc648](https://github.com/r-firth/mux/commit/5ffc6481a430d31e3bef25fa66b87f7beb6bcf62))
* Shift+h/j/k/l in Pane mode moves a pane ([3645f91](https://github.com/r-firth/mux/commit/3645f9157bd6348750e6cf3968f9ac654ac9f19d))
* Shift+Return in the palette types a command run before at the prompt ([92eaa03](https://github.com/r-firth/mux/commit/92eaa03f7f167bf6655574992a35b8f6d340c948))
* show a pane's directory in Finder from its menu or Cmd+P ([603aaef](https://github.com/r-firth/mux/commit/603aaefef79c189c7f69538b63cf7625d7a5a971))
* tabs can be dragged along the strip, and a double click renames one ([8930b0b](https://github.com/r-firth/mux/commit/8930b0bc9882d99a14e47aa2e24665d5be874523))
* the cursor is hollow while Mux is behind other windows, as in Ghostty ([508d463](https://github.com/r-firth/mux/commit/508d463d957c920000e2a76170d2e3c8c59bfbdd))
* the ground breathes around a running tab's chip instead of pulsing its dot ([957b760](https://github.com/r-firth/mux/commit/957b760c490e047423bdb12c4c169398abfba2fd))
* the I-beam over a pane's text ([ee36f4f](https://github.com/r-firth/mux/commit/ee36f4fa939c1a3fa82d34aa5dec2f6a48442ac9))
* the keys a Mac terminal answers to work beside Zellij's ([509d3eb](https://github.com/r-firth/mux/commit/509d3ebdac34fdcaa1e51fbbe7383fdfb325dfd5))
* the pane menu can even out panes, name a pane or give it a tab ([2104f6f](https://github.com/r-firth/mux/commit/2104f6fc674e6d67c41595daa9e3aa8763066686))
* the wheel follows mouse-scroll-multiplier from the Ghostty config ([d98f926](https://github.com/r-firth/mux/commit/d98f926d44e10a46bafc9946409ad3ed5e6b333c))
* the window is named for the tab on screen ([f911774](https://github.com/r-firth/mux/commit/f911774899cec508201fbbbee8f787e56915eb2d))
* the window opens where it was left ([e645308](https://github.com/r-firth/mux/commit/e64530890d6f9a017a4014620e801e5bbffcf715))
* the workspace comes back when the daemon starts again ([24ce625](https://github.com/r-firth/mux/commit/24ce6251a67aa2ea77c916867a781f67bc5f3d99))
* title from Ghostty's config fixes the window title ([366d6ff](https://github.com/r-firth/mux/commit/366d6ffea1f0302a00719567ac905505c30e4cb6))
* typing failed in Cmd+P lists the commands that failed ([f7ea538](https://github.com/r-firth/mux/commit/f7ea5381509b9657fc6263a1764269b243601b86))
* typing lets go of the selection ([ced0a3f](https://github.com/r-firth/mux/commit/ced0a3ff4eb21102cbdbdb8481cb6134df19949a))
* unfocused splits fade back, as in Ghostty ([470237c](https://github.com/r-firth/mux/commit/470237c63a45c896aa860a99dc819d058920181a))
* when nothing in the palette matches, it offers to find the text ([b9bce89](https://github.com/r-firth/mux/commit/b9bce89d5146ebefd41d58bcbe06e3fec8c8a791))
* window padding from Ghostty's config ([fef9546](https://github.com/r-firth/mux/commit/fef9546f8df4e5320471063b00500a5a0c76211e))
* window-, split- and tab-inherit-working-directory from Ghostty's config ([1cf1742](https://github.com/r-firth/mux/commit/1cf1742a49ec40bae8423471d5845535c81f930d))
* zsh and bash title their pane with where they are, as Ghostty's integration ([5020d20](https://github.com/r-firth/mux/commit/5020d2062b93b13c55e436c2f2b1bf5e4b8ba29c))


### Bug Fixes

* a directory named with escape sequences can't send them through the pane's title ([43ed60b](https://github.com/r-firth/mux/commit/43ed60bfcd6c66184f4ac6cd0655db2648185d77))
* a Mac shortcut leaves Locked mode locked ([2fac0dd](https://github.com/r-firth/mux/commit/2fac0dd81908f6b4da55e9c99bb84f1d15d986fa))
* a mode's key hints get room before a long row of tabs ([971eae7](https://github.com/r-firth/mux/commit/971eae77dcb32f5fadcd879806fa305e1c8acb37))
* a pane that comes back into view shows what it took in while hidden ([dcecebe](https://github.com/r-firth/mux/commit/dcecebebb70ea15f9ffdda3f149fda37df7126d9))
* a program is not named for the shell before it starts ([9c85c26](https://github.com/r-firth/mux/commit/9c85c262859860174cc6ff0a3ee5c3a9d9f7f337))
* a program saying it is done calls in its tab's ink, and a bell after news turns it peach ([3304116](https://github.com/r-firth/mux/commit/3304116da24e92f6b015c2fbaabfa009382567fb))
* a program waiting for in-band size reports hears about every resize ([4196228](https://github.com/r-firth/mux/commit/4196228cd6ce471adf0190a7dc65f982f53d5801))
* a program's own title names its tab and pane, not the program's process name ([4aea908](https://github.com/r-firth/mux/commit/4aea908c2af768e30f92bbf97ed53c5e28854df3))
* a prompt drawn again, as Ctrl+l does, keeps its mark ([be212ea](https://github.com/r-firth/mux/commit/be212eac0357542edc16d02841742feee7692464))
* a running tab's chip breathes brighter and wider so it shows at a glance ([2c229f5](https://github.com/r-firth/mux/commit/2c229f56c33a3363892b0ffedfa77d566ca7381d))
* a strip message gets room before a long row of tabs, as a mode's hints do ([4b1fb1c](https://github.com/r-firth/mux/commit/4b1fb1c2746cb23dcfa9a4fa704ec953f7f8ed6e))
* an empty prompt is not taken for a command ([f45e20c](https://github.com/r-firth/mux/commit/f45e20c242e161752856a852cd40eec0fdf34b10))
* back to Mux's own keys, with Ctrl+g locking as in Zellij ([8d9934f](https://github.com/r-firth/mux/commit/8d9934f5a8ee7f011e950edf3bcabae9040e324e))
* build libghostty 1.4 with Zig 0.17 and bundle its symlinked dylib ([62abdba](https://github.com/r-firth/mux/commit/62abdbaf266fb2d89a23a06dac6375138d521b09))
* clippy's assigning_clones in the palette's failed rows ([10a7fb9](https://github.com/r-firth/mux/commit/10a7fb96938cb45cf50024be59e41961f192be4f))
* closing a pane gives the keys to the pane that takes its room ([41bc245](https://github.com/r-firth/mux/commit/41bc245c7626d3e460f04667b5b778caa3417558))
* Cmd-held tab numbers take the dot's place, so the strip holds still ([94697d7](https://github.com/r-firth/mux/commit/94697d7bf922caed1a72243de62de0780d7805fb))
* Cmd+V gives a program the clipboard only while the strip still asks ([46b629b](https://github.com/r-firth/mux/commit/46b629bcf548b4d21ffa347368d8887cff2d75fd))
* failure marks in different panes keep their own hover state ([db0305d](https://github.com/r-firth/mux/commit/db0305d7c6e84ed6b1a370b65885ac0cf7d667bd))
* Ghostty keybinds with W3C key names (KeyA, Digit1, ArrowLeft) or physical: are followed, not left out ([a418a6f](https://github.com/r-firth/mux/commit/a418a6f40c40962f07f9a0298825daacf8de740a))
* Ghostty's new_split:left and new_split:up open the new pane on that side ([d424ee3](https://github.com/r-firth/mux/commit/d424ee34d6e4289442c93d0dde5cb51c33772f8e))
* Ghostty's performable: and unconsumed: keybind prefixes, so performable:ctrl+c=copy_to_clipboard still interrupts ([f901b41](https://github.com/r-firth/mux/commit/f901b410b47a710e0a00667100a0a56eed17464c))
* how commands ended survives reopening the window ([55c2600](https://github.com/r-firth/mux/commit/55c2600d3fb915d203588e3eef5c88ad4cc88a5e))
* leave Option+arrows to Zellij's pane keys ([b880e1a](https://github.com/r-firth/mux/commit/b880e1a093453e888532bf2b01566caa1c45bb78))
* letters strewn through the middles of words no longer match in Cmd+P ([e7d7514](https://github.com/r-firth/mux/commit/e7d751451962cc1a2a75bfb95d8db32dd1bac61e))
* letters typed straight after Cmd+F go into find, not the shell ([05a2f03](https://github.com/r-firth/mux/commit/05a2f037ae6c0948266f0996a4f75315c1cca519))
* letters typed straight after Cmd+P reach the palette ([39de4ef](https://github.com/r-firth/mux/commit/39de4efbca939ebee08275f17c07fb69c6ad91d0))
* letters typed straight after renaming a tab or pane are kept ([e4bb1e2](https://github.com/r-firth/mux/commit/e4bb1e26499df9d74b1c6345b78708870af246d5))
* moving the keys to a pane hidden by a zoom ends the zoom ([40f9548](https://github.com/r-firth/mux/commit/40f95485127cd369429c48b7fd229a024a9c6b8d))
* Mux no longer follows the Ghostty config beyond its theme, font and scrollback, and pastes go straight through ([e661252](https://github.com/r-firth/mux/commit/e661252773e0c4e671c116ff4b84e923dbc36aeb))
* protocol 11, so a window on libghostty 1.4 replaces a daemon still on 1.3, whose checkpoints it can't restore ([366340c](https://github.com/r-firth/mux/commit/366340ce363d8791a096b81c3236c757c844313a))
* the letter under a block cursor reads, drawn in the background's colour as Ghostty does ([888994d](https://github.com/r-firth/mux/commit/888994d6edfaa85e86e0bdceceec041d720d072b))
* the pty resizes with the terminal held, a link to another app takes a second ⌘-click, and a dropped file named with control characters is left out ([cc966ef](https://github.com/r-firth/mux/commit/cc966efa4dca0b623164a583fc1d052df91673d0))


### Performance Improvements

* hidden and closed panes let go of their images on the GPU, and libghostty is built without search and the glyph protocol, which Mux doesn't use ([e83b018](https://github.com/r-firth/mux/commit/e83b018f9247368f3fb5ea4ea08dd6e87cdd7c7c))

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
