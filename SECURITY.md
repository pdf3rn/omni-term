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
