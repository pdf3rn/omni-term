//! Variables `{{nombre}}` y `{{nombre=valor por defecto}}` dentro del contenido de un snippet.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub default: String,
}

enum Piece<'a> {
    Text(&'a str),
    Var { name: &'a str, default: &'a str },
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Divide el contenido en texto y variables. Lo que no tiene forma de variable queda como texto.
fn pieces(body: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = &after[..end];
        let (name, default) = match inner.split_once('=') {
            Some((name, default)) => (name.trim(), default.trim()),
            None => (inner.trim(), ""),
        };
        if valid_name(name) {
            if start > 0 {
                out.push(Piece::Text(&rest[..start]));
            }
            out.push(Piece::Var { name, default });
            rest = &after[end + 2..];
        } else {
            // No es una variable: se conservan las llaves y se sigue buscando después.
            out.push(Piece::Text(&rest[..start + 2]));
            rest = after;
        }
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// Variables distintas en orden de aparición. El primer valor por defecto no vacío gana.
pub fn variables(body: &str) -> Vec<Variable> {
    let mut found: Vec<Variable> = Vec::new();
    for piece in pieces(body) {
        if let Piece::Var { name, default } = piece {
            match found.iter_mut().find(|v| v.name == name) {
                Some(existing) if existing.default.is_empty() => existing.default = default.into(),
                Some(_) => {}
                None => found.push(Variable { name: name.into(), default: default.into() }),
            }
        }
    }
    found
}

/// Sustituye cada variable por su valor; si falta, usa el valor por defecto y, si no hay, queda vacía.
pub fn render(body: &str, values: &HashMap<String, String>) -> String {
    let defaults: HashMap<String, String> = variables(body).into_iter().map(|v| (v.name, v.default)).collect();
    let mut out = String::new();
    for piece in pieces(body) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Var { name, .. } => {
                let value = values.get(name).or_else(|| defaults.get(name));
                out.push_str(value.map_or("", String::as_str));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn finds_distinct_variables_in_order_with_defaults() {
        let found = variables("ssh {{user=root}}@{{host}} -p {{ port = 22 }} # {{host}}");
        assert_eq!(
            found,
            vec![
                Variable { name: "user".into(), default: "root".into() },
                Variable { name: "host".into(), default: String::new() },
                Variable { name: "port".into(), default: "22".into() },
            ]
        );
    }

    #[test]
    fn first_non_empty_default_wins_across_repeats() {
        let found = variables("{{a}} {{a=uno}} {{a=dos}}");
        assert_eq!(found, vec![Variable { name: "a".into(), default: "uno".into() }]);
    }

    #[test]
    fn renders_values_defaults_and_repeats() {
        let body = "docker logs --tail {{n=100}} {{name}}; echo {{name}}";
        assert_eq!(render(body, &values(&[("name", "web")])), "docker logs --tail 100 web; echo web");
        assert_eq!(render(body, &values(&[("name", "web"), ("n", "5")])), "docker logs --tail 5 web; echo web");
    }

    #[test]
    fn text_without_variables_is_untouched() {
        let body = "awk '{print $1}' file; echo {{ }} {{bad name}} {{";
        assert!(variables(body).is_empty());
        assert_eq!(render(body, &HashMap::new()), body);
    }

    #[test]
    fn shell_braces_do_not_confuse_the_parser() {
        let body = "echo ${HOME} {{dir=/tmp}}/{a,b}";
        assert_eq!(variables(body).len(), 1);
        assert_eq!(render(body, &values(&[("dir", "/var")])), "echo ${HOME} /var/{a,b}");
    }

    #[test]
    fn empty_value_replaces_default_only_if_provided() {
        let body = "x={{v=def}}";
        assert_eq!(render(body, &values(&[("v", "")])), "x=");
        assert_eq!(render(body, &HashMap::new()), "x=def");
    }
}
