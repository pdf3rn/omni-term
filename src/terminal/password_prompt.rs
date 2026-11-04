//! ¿Está el terminal pidiendo una contraseña? Se mira el texto de la línea del cursor.
//!
//! Sirve para no escribir una contraseña guardada en un shell normal, donde quedaría a la vista y en el
//! historial: si la línea no parece una petición de contraseña, la aplicación pide confirmación.

/// Palabras con las que los programas piden una contraseña (sudo, ssh, su, gpg, mysql, passwd…).
const KEYWORDS: [&str; 9] = ["password", "passphrase", "contraseña", "clave", "passwort", "mot de passe", "senha", "parola", "pin:"];

/// `true` si `line` (el texto antes del cursor) parece una petición de contraseña.
pub fn looks_like_password_prompt(line: &str) -> bool {
    let line = line.trim().to_lowercase();
    if line.is_empty() {
        return false;
    }
    KEYWORDS.iter().any(|keyword| line.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_usual_prompts() {
        for prompt in [
            "[sudo] password for pedro: ",
            "[sudo] contraseña para pedro: ",
            "admin@10.0.0.1's password:",
            "Password:",
            "Enter passphrase for key '/home/pedro/.ssh/id_ed25519': ",
            "Enter PIN: ",
            "Contraseña:",
            "Mot de passe : ",
        ] {
            assert!(looks_like_password_prompt(prompt), "{prompt}");
        }
    }

    #[test]
    fn a_shell_prompt_is_not_a_password_prompt() {
        for line in ["pedro@host:~$ ", "root@server:/# ", "PS C:\\Users\\pedro> ", "", "   ", "ping 10.0.0.1"] {
            assert!(!looks_like_password_prompt(line), "{line}");
        }
    }
}
