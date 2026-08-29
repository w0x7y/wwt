#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
session="$repo_root/crates/wwt/src/session.rs"
production="$(mktemp)"
trap 'rm -f "$production"' EXIT

sed '/^#\[cfg(test)\]/,$d' "$session" > "$production"

for forbidden in \
    '^    graphics: bool,$' \
    '^    pixel: bool,$' \
    '^    picture: Option<Picture>,$' \
    '^    generations: u64,$' \
    '^    fn shows_pixel' \
    '^    fn follow_focus' \
    '^    fn leave_for_a_new_tab' \
    '^    fn frame_size' \
    '^    fn on_frame'
do
    if rg -n "$forbidden" "$production"; then
        echo "pixel lifecycle leaked into production session.rs: $forbidden" >&2
        exit 1
    fi
done

cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt pixel::tests
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::switching_tabs_moves_the_screencast_with_the_focus
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::every_frame_is_acked_so_the_next_one_comes
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::cached_reader_entry_stops_pixels_and_exit_starts_them_again
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::a_resize_restarts_the_screencast_at_the_new_size
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::closing_the_focused_tab_moves_the_screencast_to_the_next_one
