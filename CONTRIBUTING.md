# Contributing

## Build and test

On Linux you need the development headers for `libxkbcommon`, `wayland`, `x11` and `dbus`.

```bash
cargo build --locked
cargo test --locked      # includes end-to-end tests against a private sshd on Linux
cargo run -- --keys      # prints received keys and the bytes sent
```

Always use `--locked`: `Cargo.lock` pins `picky` and the `smb-*` crates to versions that build together.

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
