//! Importación única de hosts desde `~/.ssh/config`.

use crate::store::{Auth, Connection};

/// Un host leído de `~/.ssh/config`; `jump_alias` es el primer salto de `ProxyJump`, aún sin resolver.
#[derive(Debug)]
pub struct ImportedHost {
    pub connection: Connection,
    pub jump_alias: Option<String>,
}

/// Lee las entradas `Host` concretas (sin comodines). Una entrada con `IdentityFile`
/// usa autenticación por llave; las demás, ssh-agent.
pub fn parse(text: &str, default_user: &str) -> Vec<ImportedHost> {
    struct Entry {
        alias: String,
        host: Option<String>,
        user: Option<String>,
        port: Option<u16>,
        identity: Option<String>,
        jump: Option<String>,
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Vec<Entry> = Vec::new();

    let flush = |current: &mut Vec<Entry>, entries: &mut Vec<Entry>| entries.append(current);

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((key, value)) => (key.to_ascii_lowercase(), value.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        let value = value.trim_matches('"');

        match key.as_str() {
            "host" => {
                flush(&mut current, &mut entries);
                current = value
                    .split_whitespace()
                    .filter(|alias| !alias.contains(['*', '?', '!']))
                    .map(|alias| Entry { alias: alias.into(), host: None, user: None, port: None, identity: None, jump: None })
                    .collect();
            }
            "match" => {
                flush(&mut current, &mut entries);
            }
            "hostname" => current.iter_mut().for_each(|e| e.host = Some(value.into())),
            "user" => current.iter_mut().for_each(|e| e.user = Some(value.into())),
            "port" => {
                if let Ok(port) = value.parse() {
                    current.iter_mut().for_each(|e| e.port = Some(port));
                }
            }
            "proxyjump" => {
                // Solo se importa el primer salto, sin `usuario@` ni `:puerto`.
                let first = value.split(',').next().unwrap_or("").trim();
                let alias = first.rsplit('@').next().unwrap_or(first).split(':').next().unwrap_or(first);
                if !alias.is_empty() && !alias.eq_ignore_ascii_case("none") {
                    current.iter_mut().for_each(|e| e.jump = Some(alias.to_string()));
                }
            }
            "identityfile" => current.iter_mut().for_each(|e| {
                e.identity.get_or_insert_with(|| value.into());
            }),
            _ => {}
        }
    }
    flush(&mut current, &mut entries);

    entries
        .into_iter()
        .map(|entry| ImportedHost {
            connection: Connection {
                id: None,
                name: entry.alias.clone(),
                host: entry.host.unwrap_or(entry.alias),
                port: entry.port.unwrap_or(22),
                user: entry.user.unwrap_or_else(|| default_user.into()),
                auth: if entry.identity.is_some() { Auth::Key } else { Auth::Agent },
                key_path: entry.identity,
                init_command: None,
                jump: None,
                color: None,
                env: None,
            },
            jump_alias: entry.jump,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_concrete_hosts_and_skips_wildcards() {
        let text = "
# comentario
Host *
    ServerAliveInterval 30

Host web prod
    HostName 10.0.0.5
    User deploy
    Port 2200
    IdentityFile ~/.ssh/id_prod

Host db
    HostName db.example.com

Host *.internal
    User nobody
";
        let found: Vec<_> = parse(text, "me").into_iter().map(|h| h.connection).collect();
        assert_eq!(found.len(), 3);

        assert_eq!(found[0].name, "web");
        assert_eq!(found[0].host, "10.0.0.5");
        assert_eq!(found[0].user, "deploy");
        assert_eq!(found[0].port, 2200);
        assert_eq!(found[0].auth, Auth::Key);
        assert_eq!(found[0].key_path.as_deref(), Some("~/.ssh/id_prod"));
        assert_eq!(found[1].name, "prod");

        assert_eq!(found[2].name, "db");
        assert_eq!(found[2].host, "db.example.com");
        assert_eq!(found[2].user, "me");
        assert_eq!(found[2].port, 22);
        assert_eq!(found[2].auth, Auth::Agent);
    }

    #[test]
    fn host_without_hostname_uses_alias_and_equals_syntax_works() {
        let found: Vec<_> = parse("Host=box\n  User=root\n", "me").into_iter().map(|h| h.connection).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "box");
        assert_eq!(found[0].user, "root");
    }

    #[test]
    fn match_blocks_do_not_leak_settings_into_previous_hosts() {
        let found: Vec<_> = parse("Host a\n  HostName a.test\nMatch host b\n  HostName wrong\n", "me").into_iter().map(|h| h.connection).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "a.test");
    }

    #[test]
    fn proxy_jump_keeps_only_the_first_hop_alias() {
        let found = parse("Host interno\n  ProxyJump deploy@bastion:2222,otro\nHost directo\n  ProxyJump none\n", "me");
        assert_eq!(found[0].jump_alias.as_deref(), Some("bastion"));
        assert_eq!(found[1].jump_alias, None);
    }
}
