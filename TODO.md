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
