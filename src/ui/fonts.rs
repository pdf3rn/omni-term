//! Fuentes monoespaciadas instaladas en el equipo, para elegir la del terminal.

use std::sync::OnceLock;

use fontdb::{Database, Family, Query, Stretch, Style, Weight};

fn database() -> &'static Database {
    static DB: OnceLock<Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    })
}

/// Empieza a leer las fuentes del sistema en segundo plano para que la lista esté lista al abrir los ajustes.
pub fn preload() {
    std::thread::spawn(|| {
        monospace_families();
    });
}

/// Familias monoespaciadas instaladas, sin repetir y por orden alfabético.
pub fn monospace_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut names: Vec<String> = database()
            .faces()
            .filter(|face| face.monospaced)
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    })
}

/// Una variante de una familia: bytes del archivo y número de la fuente dentro de él (las colecciones `.ttc` tienen varias).
pub struct Face {
    pub data: Vec<u8>,
    pub index: u32,
}

/// Las cuatro variantes del terminal (normal, negrita, cursiva y negrita cursiva) de una familia instalada.
/// Si la familia no tiene alguna, se usa la más parecida que exista.
pub fn load_family(name: &str) -> Result<[Face; 4], String> {
    let db = database();
    let families = [Family::Name(name)];
    let find = |weight: Weight, style: Style| {
        let id = db.query(&Query { families: &families, weight, stretch: Stretch::Normal, style })?;
        db.with_face_data(id, |data, index| Face { data: data.to_vec(), index })
    };
    let regular = find(Weight::NORMAL, Style::Normal).ok_or_else(|| format!("No se encontró la fuente «{name}»"))?;
    let bold = find(Weight::BOLD, Style::Normal).unwrap_or_else(|| Face { data: regular.data.clone(), index: regular.index });
    let italic = find(Weight::NORMAL, Style::Italic).or_else(|| find(Weight::NORMAL, Style::Oblique)).unwrap_or_else(|| Face { data: regular.data.clone(), index: regular.index });
    let bold_italic = find(Weight::BOLD, Style::Italic)
        .or_else(|| find(Weight::BOLD, Style::Oblique))
        .unwrap_or_else(|| Face { data: bold.data.clone(), index: bold.index });
    Ok([regular, bold, italic, bold_italic])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_family_reports_its_name() {
        let error = load_family("Fuente Que No Existe 12345").err().unwrap();
        assert!(error.contains("Fuente Que No Existe 12345"));
    }

    #[test]
    fn listed_families_can_be_loaded_with_four_variants() {
        // Cualquier equipo con una fuente monoespaciada instalada la ofrece; sin ninguna, no hay nada que comprobar.
        if let Some(name) = monospace_families().first() {
            let faces = load_family(name).unwrap();
            assert!(faces.iter().all(|f| ab_glyph::FontRef::try_from_slice_and_index(&f.data, f.index).is_ok()));
        }
    }
}
