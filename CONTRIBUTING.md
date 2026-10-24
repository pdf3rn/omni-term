# Contributing

## Build and test

On Linux you need the development headers for `libxkbcommon`, `wayland`, `x11` and `dbus`.

```bash
cargo build --locked
cargo test --locked      # includes end-to-end tests against a private sshd on Linux
cargo run -- --keys      # prints received keys and the bytes sent
```

Always use `--locked`: `Cargo.lock` pins `picky` and the `smb-*` crates to versions that build together.

## Code layout

All code lives under `src/`, grouped by domain. Each folder has a sibling file (for example `terminal.rs` next to `terminal/`) that declares its modules.

| Folder | What it holds |
| --- | --- |
| `terminal/` | Terminal state and rendering: `term`, `pane`, `keys`, `mouse`, `search`, `layout`, `snippet_vars`. |
| `connect/` | Terminal connections: `ssh`, `telnet`, `serial`, `local`, plus `socks`, `known_hosts`, `sshconfig`. |
| `files/` | Remote files: the explorer, transfers, and the SFTP, FTP, SMB and ADB clients. |
| `containers/` | Docker and Podman manager: the engine client and its view. |
| `data/` | Storage: the SQLite `store`, `secrets` and `settings`. |
| `ui/` | The window and everything drawn. `ui.rs` holds the `App` state and the frame loop; its methods live in topic modules (`sessions`, `connecting`, `secrets_flow`, `library`, `commands`, `closing`, `startup`, `dialog_flow`, `toasts`), and the screens in `sidebar`, `tabs`, `empty`, `window` (title bar) and `appearance`. Also `dialogs/` (shell, connection form, settings, prompts), `palette`, `theme`, `icons`, `fonts` and the reusable `kit`. |

## UI style

Everything that looks like something lives in one of two places:

- `src/ui/theme.rs`: colors, radii (`theme::radius`), spacing (`theme::space`), text and icon sizes (`theme::size`), stroke widths (`theme::stroke`) and fixed heights (`theme::height`).
- `src/ui/kit/`: reusable components (buttons with `Variant`, cards and settings rows, list rows, form fields, chips, keycaps).

To change how buttons, cards or rows look, edit those files. Do not write numeric radii, strokes, font sizes, spacing or `Color32::from_rgb(..)` in the screens: a test (`style_guard`) fails if you do.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`… Add `!` or a `BREAKING CHANGE:` footer for incompatible changes. The commit type decides the next version.

## Releases

Versions follow [Semantic Versioning](https://semver.org/) and are automatic:

1. Every push to `main` makes [release-plz](https://release-plz.dev) open or update a `chore(release)` pull request with the new version and the `CHANGELOG.md` entry.
2. Merging that pull request creates the `vX.Y.Z` tag.
3. The tag runs `release.yml`, which builds Linux, Windows and macOS binaries and publishes them.

Maintainers need a `RELEASE_PLZ_TOKEN` repository secret (a personal token with contents and pull-request write access). With the default `GITHUB_TOKEN` the tag would not trigger the release workflow. The release workflow fails if the tag does not match the version in `Cargo.toml`.

## License

By contributing you agree that your work is licensed under `MIT OR Apache-2.0`, like the rest of the project.
