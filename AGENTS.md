# Working on Roughcut

Roughcut is a Rust/GPUI macOS app. The executable is `roughcut`; the Cargo package remains `quill`.

## UI changes and README images

After any change to visible UI, layout, styling, icons, or demo content:

1. Run `python3 scripts/capture-readme.py` on macOS with a graphical login session.
2. Open and visually inspect all four generated images in `docs/images/`: `library.png`, `page.png`, `board.png`, and `focus.png`. Fix clipping, unintended overlaps, or other regressions and recapture as needed.
3. Include the refreshed images with the UI changes. Keep README image links accurate and its prose terse.

The capture tool builds the screenshot feature, runs the real app using a fresh temporary demo library, disables music polling, and exits automatically. It needs neither real documents nor Screen Recording permission. Never substitute screenshots of a personal library or live music playback. If capture is unavailable, report why and explicitly state that README images were not refreshed; do not claim they are current.

The banner is editable vector artwork at `docs/images/banner.svg`; update it when the branding changes.

## Validation

- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets --features screenshots -- -D warnings`
- Library tests: `cargo test --workspace --lib`
- Editor interactions: `cargo test -p quill --features screenshots --test editor` (macOS graphics session required).

Run checks appropriate to the change. Preserve uncommitted user work and keep generated libraries, secrets, and build products out of Git.
