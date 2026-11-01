//! Componentes reutilizables de la interfaz. Todo lo que se ve se define aquí a partir de los valores
//! de `theme` (colores, radios, espaciado, tamaños): para cambiar el estilo se toca este módulo o el tema.

pub mod button;
pub mod card;
pub mod form;
pub mod list;

pub use button::{button, button_rich, button_with, icon_button, IconButton, Variant};
pub use card::{card, card_frame, keycap, section_title, setting_row, toggle, switch, switch_labeled};
pub use list::{connection_glyph, empty_hint, fit_text, row_content, row_frame, Leading, RowContent};
pub use form::{chip, error_line, form_row, password_field, request_reveal_error, search_field, take_reveal_error, text_input};

#[cfg(test)]
mod style_guard {
    //! Guardián del estilo: fuera de `ui/theme.rs` y de `ui/kit/` no puede haber colores, radios, grosores, tamaños de
    //! letra ni espacios escritos como números sueltos. Si falla, usa un valor de `theme` o añade un componente al kit.

    use std::path::{Path, PathBuf};

    /// Fragmentos que, seguidos de un número, indican un valor de estilo suelto.
    const NUMERIC_CALLS: [&str; 5] = ["CornerRadius::same(", "Stroke::new(", "FontId::new(", "FontId::proportional(", "add_space("];
    /// Colores escritos a mano.
    const COLOR_LITERALS: [&str; 4] = ["Color32::from_rgb(", "Color32::from_rgba", "Color32::from_gray(", "Color32::from_black_alpha("];

    fn sources() -> Vec<PathBuf> {
        let mut files = Vec::new();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        files
    }

    /// Código de producción: sin el módulo de pruebas del final del archivo.
    fn production(source: &str) -> &str {
        source.split("#[cfg(test)]").next().unwrap_or(source)
    }

    fn starts_with_number(rest: &str) -> bool {
        let rest = rest.trim_start_matches('-');
        rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    }

    #[test]
    fn style_values_come_from_the_theme_or_the_kit() {
        let mut problems = Vec::new();
        for path in sources() {
            let name = path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().display().to_string().replace('\\', "/");
            if name.ends_with("ui/theme.rs") || name.contains("ui/kit") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for (number, line) in production(&source).lines().enumerate() {
                for call in NUMERIC_CALLS {
                    let mut rest = line;
                    while let Some(at) = rest.find(call) {
                        rest = &rest[at + call.len()..];
                        if starts_with_number(rest) {
                            problems.push(format!("{name}:{}: número suelto en `{call}`", number + 1));
                        }
                    }
                }
                for literal in COLOR_LITERALS {
                    if line.contains(literal) {
                        problems.push(format!("{name}:{}: color escrito a mano (`{literal}`)", number + 1));
                    }
                }
            }
        }
        assert!(problems.is_empty(), "valores de estilo fuera del tema:\n{}", problems.join("\n"));
    }
}
