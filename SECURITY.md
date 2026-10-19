# Security policy

omni-term stores SSH, FTP and SMB credentials and opens network connections, so security reports matter.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private reporting: **Security → Report a vulnerability** on <https://github.com/pdf3rn/omni-term>.

Include the version (`omni-term --version`), your operating system, and steps to reproduce. You can expect a first answer within a week.

## Supported versions

Only the latest release receives fixes while the project is below 1.0.

## Known limits

- FTP without TLS sends credentials in clear text. omni-term warns before connecting and marks the connection with a lock.
- Telnet is also unencrypted and gets the same warning.
- FTPS and SMB certificates and signing are checked by the underlying libraries; self-signed FTPS certificates are rejected.

## Threat model

omni-term protects your credentials from other users of the machine and from someone who copies your files. It does not protect them from malware running as you.

**Where things are stored**
- **System keyring** (default): passwords and passphrases are held by the operating system's secret service and unlocked with your login session. Any program running as your user can ask for them, subject to the keyring's own prompts.
- **Master key** (fallback, or forced in Settings): secrets are encrypted in the local database with XChaCha20-Poly1305 using a key derived from your master key with Argon2id. Without the master key, the database alone does not reveal them. Once unlocked, the key stays in memory until you close omni-term.
- **Local database** (`omniterm.db`): host names, user names, connection profiles, tunnels and snippets are stored unencrypted. On Unix the data directory is created with mode `0700` and the database with `0600`. On Windows it relies on the permissions of your user profile.
- **Decrypted secrets** live in process memory for as long as the application runs (so a tab can reconnect) and are zeroed when dropped. omni-term does not write them to logs.

**If your operating-system account is compromised**, assume everything omni-term can read is exposed, including the keyring contents and an unlocked master key.

**Host keys**
- Unknown hosts require your confirmation and are saved to `~/.ssh/known_hosts` by replacing the file atomically (new files get mode `0600`; existing permissions and symlinks are kept).
- If the host key cannot be saved, the connection is cancelled with an error instead of continuing without a record.
- A changed host key always cancels the connection.

**Tunnels**
- New local (`-L`) and SOCKS (`-D`) tunnels listen on `127.0.0.1`. The connection editor warns when a tunnel listens on any other address, because other devices on the network could then use it. Remote (`-R`) tunnels are bound by the server, so omni-term cannot warn about them.

## Decisions

These suggestions were considered and intentionally not implemented, to keep the application simple:

- **Application lock or re-authentication** when using saved credentials. Use the master key mode if you want a secret that is not tied to your login session.
- **Disabling FTP and Telnet by default or restricting protocols by policy.** They are marked with a lock and need a confirmation before connecting.
- **Tunnel event logging.**

## Verifying a release

Each release publishes a `SHA256SUMS` file and a build-provenance attestation:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify omni-term-*.tar.gz --repo pdf3rn/omni-term
```

Binaries are not signed with a private key.
