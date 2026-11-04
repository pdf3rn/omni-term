//! Modificadores de los atajos de la aplicación según el sistema.
//!
//! En Linux y Windows los atajos propios van con Ctrl+Shift: Ctrl solo llega a los programas del terminal.
//! En macOS van con Cmd, que el terminal no usa (Control sigue llegando a los programas).
//! Los textos se escriben con la forma de Linux y Windows; `label` los traduce para macOS.

use eframe::egui::{Key, Modifiers};

pub const MAC: bool = cfg!(target_os = "macos");

/// Atajos propios: Ctrl+Shift+letra (en macOS, Cmd+letra).
pub fn app() -> Modifiers {
    if MAC { Modifiers::MAC_CMD } else { Modifiers::CTRL | Modifiers::SHIFT }
}

/// ¿Lleva `modifiers` el modificador de los atajos propios?
pub fn is_app(modifiers: Modifiers) -> bool {
    if MAC { modifiers.mac_cmd && !modifiers.ctrl && !modifiers.alt } else { modifiers.ctrl && modifiers.shift }
}

/// Ctrl (en macOS, Cmd): ajustes, tamaño de letra y explorador de archivos.
pub fn primary() -> Modifiers {
    if MAC { Modifiers::MAC_CMD } else { Modifiers::CTRL }
}

/// Escribir una contraseña guardada: Ctrl+Alt+P (en macOS, Cmd+Option+P).
pub fn password() -> Modifiers {
    if MAC { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::CTRL | Modifiers::ALT }
}

/// Ir a la pestaña 1…9: Alt+número (en macOS, Cmd+número).
pub fn tab_digit() -> Modifiers {
    if MAC { Modifiers::MAC_CMD } else { Modifiers::ALT }
}

/// Ir al panel de al lado: Alt+flechas (en macOS, Cmd+Option+flechas: Option+flechas mueve por palabras).
pub fn pane_move() -> Modifiers {
    if MAC { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::ALT }
}

/// Mover el borde del panel: Alt+Shift+flechas (en macOS, Cmd+Ctrl+flechas).
pub fn pane_nudge() -> Modifiers {
    if MAC { Modifiers::MAC_CMD | Modifiers::CTRL } else { Modifiers::ALT | Modifiers::SHIFT }
}

/// Ampliar o restaurar el panel: Ctrl+Shift+Z (en macOS, Cmd+Shift+Enter: Cmd+Z es deshacer).
pub fn pane_zoom() -> (Modifiers, Key) {
    if MAC { (Modifiers::MAC_CMD | Modifiers::SHIFT, Key::Enter) } else { (Modifiers::CTRL | Modifiers::SHIFT, Key::Z) }
}

/// Texto de un atajo tal como se muestra en este sistema.
pub fn label(text: &str) -> String {
    label_for(text, MAC)
}

/// Traduce un atajo escrito para Linux y Windows a su forma en macOS (si `mac`).
pub fn label_for(text: &str, mac: bool) -> String {
    if !mac {
        return text.to_string();
    }
    // Ctrl+Tab no cambia en macOS: se protege antes de convertir el resto de Ctrl.
    const KEEP: [(&str, &str); 2] = [("Ctrl+Shift+Tab", "\u{1}"), ("Ctrl+Tab", "\u{2}")];
    let mut text = text.to_string();
    for (from, mark) in KEEP {
        text = text.replace(from, mark);
    }
    for (from, to) in [
        ("Ctrl+Shift+Z", "Cmd+Shift+Enter"),
        // El Ctrl de la salida se marca para que la regla general de Ctrl no lo convierta.
        ("Alt+Shift+flechas", "Cmd+\u{3}+flechas"),
        ("Alt+flechas", "Cmd+Option+flechas"),
        ("Alt+Enter", "Option+Enter"),
        ("Ctrl+Alt+", "Cmd+Option+"),
        ("Ctrl+Shift+", "Cmd+"),
        ("Ctrl+", "Cmd+"),
    ] {
        text = text.replace(from, to);
    }
    // Alt+número (pestañas) pasa a Cmd+número.
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Alt+") {
        let after = &rest[at + 4..];
        out.push_str(&rest[..at]);
        out.push_str(if after.starts_with(|c: char| c.is_ascii_digit()) { "Cmd+" } else { "Alt+" });
        rest = after;
    }
    out.push_str(rest);
    for (from, mark) in KEEP {
        out = out.replace(mark, from);
    }
    out.replace('\u{3}', "Ctrl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_and_windows_labels_stay_as_written() {
        assert_eq!(label_for("Ctrl+Shift+C / V", false), "Ctrl+Shift+C / V");
        assert_eq!(label_for("Alt+1 … 9", false), "Alt+1 … 9");
    }

    #[test]
    fn mac_labels_use_cmd() {
        let mac = |text| label_for(text, true);
        assert_eq!(mac("Ctrl+Shift+C / V"), "Cmd+C / V");
        assert_eq!(mac("Ctrl+Shift+P"), "Cmd+P");
        assert_eq!(mac("Ctrl+,"), "Cmd+,");
        assert_eq!(mac("Ctrl+= / Ctrl+− / Ctrl+0"), "Cmd+= / Cmd+− / Cmd+0");
        assert_eq!(mac("Ctrl+Alt+P"), "Cmd+Option+P");
        assert_eq!(mac("Alt+1 … 9"), "Cmd+1 … 9");
        assert_eq!(mac("Ir a la pestaña 1 … 9 (Alt+9, la última)"), "Ir a la pestaña 1 … 9 (Cmd+9, la última)");
        assert_eq!(mac("Alt+flechas"), "Cmd+Option+flechas");
        assert_eq!(mac("Alt+Shift+flechas"), "Cmd+Ctrl+flechas");
        assert_eq!(mac("Ctrl+Shift+Z"), "Cmd+Shift+Enter");
        assert_eq!(mac("Alt+Enter"), "Option+Enter");
        assert_eq!(mac("Ctrl+Enter"), "Cmd+Enter");
        assert_eq!(mac("Ctrl+Tab / Ctrl+Shift+Tab"), "Ctrl+Tab / Ctrl+Shift+Tab");
        assert_eq!(mac("Cerrar panel  (Ctrl+Shift+W)"), "Cerrar panel  (Cmd+W)");
    }
}
