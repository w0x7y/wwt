# TODO

## Nice To Have Features

### Appearance and themes

- [ ] Add `userChrome.css` integration for user-defined browser styling.
- [ ] Add a dark, light, and system color-mode switcher.
- [ ] Support custom UI themes with configurable foreground, background, accent, status, error, hint, and tab colors.
- [ ] Allow separate themes for normal, insert, hint, command, reader, and pixel modes.

### Configuration

- [ ] Add more configuration options:
  - `homepage` for new tabs and empty sessions.
  - `restore_session` to control whether tabs reopen at startup.
  - `default_view` to select text, reader, or pixel mode.
  - `color_mode` to select dark, light, or system colors.
  - `theme` to select a custom theme.
  - `mouse` to enable or disable mouse capture at startup.
  - `scroll_lines` to set the number of rows moved by the mouse wheel.
  - `download_dir` to select where downloads are saved.
  - `confirm_close_many_tabs` to prevent accidental session loss.
- [ ] Support per-site configuration overrides.
- [ ] Reload the configuration without restarting the browser.
- [ ] Allow users to remap keys and define command aliases.

### Commands

- [ ] Add commands that make common tasks faster:
  - `:help` to list keys and commands.
  - `:config` to show the active configuration and its file path.
  - `:config-reload` to reload the configuration.
  - `:theme <name>` to switch themes.
  - `:tabs` to list and select open tabs.
  - `:tabmove <position>` to reorder the current tab.
  - `:duplicate` to duplicate the current tab.
  - `:find <text>` and `:findnext` to search the current page.
  - `:yank` to copy the current URL.
  - `:bookmark` and `:bookmarks` to save and open bookmarks.
  - `:history` to search browsing history.
  - `:downloads` to show active and completed downloads.
  - `:view-source` to open the current page source.
- [ ] Add command history, completion, and suggestions.
- [ ] Improve `:login`:
  - Use a minimal Chromium window, possibly with app mode.
  - Detect login completion automatically where reliable.
  - Add `:login finish` to close the login browser and return to WWT.

### Browsing

- [ ] Add find-in-page with highlighted matches and next or previous navigation.
- [ ] Add bookmarks with folders, tags, and import or export support.
- [ ] Add searchable browsing history with configurable retention.
- [ ] Add a download manager with progress, cancellation, and retry.
- [ ] Add optional ad and tracker blocking with user-managed filter lists.
- [ ] Add a permission manager for notifications, location, camera, microphone, and clipboard access.
- [ ] Add page zoom controls that preserve the terminal coordinate model.
- [ ] Add a split view for displaying two tabs at once.
- [ ] Add tab pinning and recently closed tab recovery.
- [ ] Add a private browsing command that opens an isolated tab or window.

### Extensibility and accessibility

- [ ] Add user scripts that can run globally or on matching sites.
- [ ] Provide an extension API for custom commands and page actions.
- [ ] Add configurable high-contrast themes and color-blind-safe hint palettes.
- [ ] Add optional text-to-speech for reader mode.
- [ ] Add an export command for saving reader content as plain text or Markdown.

## Possible fixes to the YouTube problem

The failure occurs in pixel mode: video frames advance while YouTube's loading bar remains visible and the surrounding page UI stays incomplete or unresponsive. These candidates are ranked from most likely to least likely. Confirm the cause with a reproducible test before implementing a fix.

- [ ] Restore Chromium's normal frame pacing instead of launching every page with `--disable-frame-rate-limit`.
- [ ] Let Chromium use the GPU instead of launching every page with `--disable-gpu`.
- [ ] Reduce the pixel-mode screencast frame rate while full-motion video is playing.
- [ ] Adapt screencast resolution to terminal throughput instead of encoding every video frame at the full terminal pixel size.
- [ ] Use a cheaper screencast format for moving video while keeping PNG for pages where sharp text matters.
- [ ] Apply screencast backpressure before Chromium spends time encoding a frame that the terminal cannot display yet.
- [ ] Keep at most one screencast acknowledgement task in flight for each page.
- [ ] Stop pixel-mode dirty signals from starting repeated status reads while YouTube mutates its player controls.
- [ ] Filter player mutations that do not change WWT's title, URL, or scroll status.
- [ ] Cap the rate of status reads caused by continuous YouTube mutations.
- [ ] Measure YouTube's renderer main thread and lower WWT's workload when long tasks block page hydration.
- [ ] Run WWT's bootstrap in an isolated JavaScript world so its observer and globals cannot interfere with YouTube.
- [ ] Replace the page-global `window.__wwt` object and `__wwt_dirty` binding with names that cannot collide with site code.
- [ ] Add a comparison mode that loads the page without WWT's bootstrap to identify injection-related failures.
- [ ] Prioritize CDP command responses over large `Page.screencastFrame` events so status and input calls cannot starve.
- [ ] Keep synchronous terminal writes from delaying browser events and screencast acknowledgements.
- [ ] Detect a stalled `Page.startScreencast` pipeline and restart it without reloading the page.
- [ ] Send user-agent client hints that match the user-agent string WWT reports to YouTube.
- [ ] Detect YouTube's headless or automation challenge and report it instead of leaving a half-hydrated page.
- [ ] Check YouTube's console for an uncaught hydration error and reload only the failed application shell.
- [ ] Detect failed YouTube application API requests separately from the working `googlevideo.com` media stream.
- [ ] Detect an advertisement request that leaves the watch-page application waiting after video playback begins.
- [ ] Add a targeted reset for YouTube's service worker, cache, IndexedDB, local storage, and cookies.
- [ ] Add a clean-profile comparison for YouTube without changing the user's persistent profile.
- [ ] Add a safe comparison run without extensions, content filters, or profile policies.
- [ ] Check the Chromium version and report known headless rendering or YouTube compatibility failures.
- [ ] Prefer a video codec that leaves enough CPU time for YouTube's page UI.
- [ ] Lower YouTube playback quality when software decoding saturates the CPU.
- [ ] Preserve YouTube's expected viewport, focus, and visibility state while pixel mode is active.
- [ ] Detect account experiments, consent flows, regional responses, and temporary YouTube application failures that leave the page shell unfinished.
