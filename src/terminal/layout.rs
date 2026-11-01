//! Árbol de divisiones de una pestaña: hojas (paneles) y divisiones horizontales o verticales.

use eframe::egui::{Pos2, Rect, Vec2};

const MIN_RATIO: f32 = 0.15;
const MAX_RATIO: f32 = 0.85;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Dos paneles lado a lado.
    Horizontal,
    /// Un panel encima del otro.
    Vertical,
}

enum Node<T> {
    Leaf { id: u64, item: T },
    Split { dir: Dir, ratio: f32, a: Box<Node<T>>, b: Box<Node<T>> },
}

/// Sentido en el que se mueve el foco entre paneles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// El panel vecino de `from` en el sentido `side`, según la posición de cada hoja en pantalla.
/// Solo cuentan los paneles que quedan de ese lado y comparten tramo con `from`; de ellos gana el más cercano
/// y, a igual distancia, el más alineado con el centro de `from`. Sin vecino devuelve `None`.
pub fn neighbor(leaves: &[(u64, Rect)], from: u64, side: Side) -> Option<u64> {
    let (_, current) = leaves.iter().find(|(id, _)| *id == from)?;
    // Un panel está «al lado» si empieza donde termina el actual (con un margen por redondeos) y se solapa en el otro eje.
    let tolerance = 1.0;
    leaves
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, rect)| {
            let (distance, overlap, offset) = match side {
                Side::Right => (rect.min.x - current.max.x, overlap(current.y_range(), rect.y_range()), (rect.center().y - current.center().y).abs()),
                Side::Left => (current.min.x - rect.max.x, overlap(current.y_range(), rect.y_range()), (rect.center().y - current.center().y).abs()),
                Side::Down => (rect.min.y - current.max.y, overlap(current.x_range(), rect.x_range()), (rect.center().x - current.center().x).abs()),
                Side::Up => (current.min.y - rect.max.y, overlap(current.x_range(), rect.x_range()), (rect.center().x - current.center().x).abs()),
            };
            (distance >= -tolerance && overlap > tolerance).then_some((*id, distance.max(0.0), offset))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(id, _, _)| id)
}

/// Longitud del tramo común entre dos rangos.
fn overlap(a: eframe::egui::Rangef, b: eframe::egui::Rangef) -> f32 {
    a.max.min(b.max) - a.min.max(b.min)
}

/// Asa para redimensionar una división.
#[derive(Clone, Debug, PartialEq)]
pub struct Handle {
    /// Camino desde la raíz hasta la división (`false` = primer hijo, `true` = segundo).
    pub path: Vec<bool>,
    pub rect: Rect,
    pub dir: Dir,
    /// Rectángulo completo de la división, para convertir el puntero en proporción.
    pub parent: Rect,
}

impl Handle {
    pub fn ratio_at(&self, pointer: Pos2) -> f32 {
        let ratio = match self.dir {
            Dir::Horizontal => (pointer.x - self.parent.min.x) / self.parent.width(),
            Dir::Vertical => (pointer.y - self.parent.min.y) / self.parent.height(),
        };
        ratio.clamp(MIN_RATIO, MAX_RATIO)
    }
}

/// Un árbol con al menos una hoja.
pub struct Tree<T> {
    root: Option<Node<T>>,
}

impl<T> Tree<T> {
    pub fn new(id: u64, item: T) -> Self {
        Self { root: Some(Node::Leaf { id, item }) }
    }

    fn root(&self) -> &Node<T> {
        self.root.as_ref().expect("el árbol nunca queda vacío")
    }

    pub fn ids(&self) -> Vec<u64> {
        let mut ids = Vec::new();
        self.for_each(&mut |id, _| ids.push(id));
        ids
    }

    pub fn len(&self) -> usize {
        self.ids().len()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: u64) -> Option<&T> {
        fn go<T>(node: &Node<T>, wanted: u64) -> Option<&T> {
            match node {
                Node::Leaf { id, item } => (*id == wanted).then_some(item),
                Node::Split { a, b, .. } => go(a, wanted).or_else(|| go(b, wanted)),
            }
        }
        go(self.root(), id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        fn go<T>(node: &mut Node<T>, wanted: u64) -> Option<&mut T> {
            match node {
                Node::Leaf { id, item } => (*id == wanted).then_some(item),
                Node::Split { a, b, .. } => match go(a, wanted) {
                    Some(found) => Some(found),
                    None => go(b, wanted),
                },
            }
        }
        go(self.root.as_mut().expect("el árbol nunca queda vacío"), id)
    }

    pub fn for_each(&self, f: &mut impl FnMut(u64, &T)) {
        fn go<T>(node: &Node<T>, f: &mut impl FnMut(u64, &T)) {
            match node {
                Node::Leaf { id, item } => f(*id, item),
                Node::Split { a, b, .. } => {
                    go(a, f);
                    go(b, f);
                }
            }
        }
        go(self.root(), f);
    }

    /// Divide la hoja `target`: se queda en un lado y la nueva (`new_id`) va al otro.
    /// Devuelve `false`, sin cambios, si `target` no existe.
    pub fn split(&mut self, target: u64, dir: Dir, new_id: u64, item: T) -> bool {
        if !self.contains(target) {
            return false;
        }
        fn go<T>(node: Node<T>, target: u64, dir: Dir, new_id: u64, item: &mut Option<T>) -> Node<T> {
            match node {
                Node::Leaf { id, item: old } if id == target => {
                    let new = item.take().expect("el elemento nuevo se usa una sola vez");
                    Node::Split {
                        dir,
                        ratio: 0.5,
                        a: Box::new(Node::Leaf { id, item: old }),
                        b: Box::new(Node::Leaf { id: new_id, item: new }),
                    }
                }
                leaf @ Node::Leaf { .. } => leaf,
                Node::Split { dir: d, ratio, a, b } => Node::Split {
                    dir: d,
                    ratio,
                    a: Box::new(go(*a, target, dir, new_id, item)),
                    b: Box::new(go(*b, target, dir, new_id, item)),
                },
            }
        }
        let root = self.root.take().expect("el árbol nunca queda vacío");
        self.root = Some(go(root, target, dir, new_id, &mut Some(item)));
        true
    }

    /// Quita la hoja `target`; su hermana ocupa su lugar. Devuelve `false` si es la única
    /// hoja o no existe: el árbol no puede quedar vacío.
    pub fn remove(&mut self, target: u64) -> bool {
        if self.len() <= 1 || !self.contains(target) {
            return false;
        }
        fn go<T>(node: Node<T>, target: u64) -> Option<Node<T>> {
            match node {
                Node::Leaf { id, .. } if id == target => None,
                leaf @ Node::Leaf { .. } => Some(leaf),
                Node::Split { dir, ratio, a, b } => match (go(*a, target), go(*b, target)) {
                    (Some(a), Some(b)) => Some(Node::Split { dir, ratio, a: Box::new(a), b: Box::new(b) }),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                },
            }
        }
        let root = self.root.take().expect("el árbol nunca queda vacío");
        self.root = go(root, target);
        true
    }

    /// Reparte `area` entre las hojas, dejando `gap` entre paneles. Devuelve la zona de cada
    /// hoja y las asas de las divisiones.
    pub fn layout(&self, area: Rect, gap: f32) -> (Vec<(u64, Rect)>, Vec<Handle>) {
        fn go<T>(node: &Node<T>, area: Rect, gap: f32, path: &mut Vec<bool>, out: &mut (Vec<(u64, Rect)>, Vec<Handle>)) {
            match node {
                Node::Leaf { id, .. } => out.0.push((*id, area)),
                Node::Split { dir, ratio, a, b } => {
                    let (first, second, handle) = match dir {
                        Dir::Horizontal => {
                            let cut = area.min.x + area.width() * ratio;
                            (
                                Rect::from_min_max(area.min, Pos2::new(cut - gap / 2.0, area.max.y)),
                                Rect::from_min_max(Pos2::new(cut + gap / 2.0, area.min.y), area.max),
                                Rect::from_center_size(Pos2::new(cut, area.center().y), Vec2::new(gap, area.height())),
                            )
                        }
                        Dir::Vertical => {
                            let cut = area.min.y + area.height() * ratio;
                            (
                                Rect::from_min_max(area.min, Pos2::new(area.max.x, cut - gap / 2.0)),
                                Rect::from_min_max(Pos2::new(area.min.x, cut + gap / 2.0), area.max),
                                Rect::from_center_size(Pos2::new(area.center().x, cut), Vec2::new(area.width(), gap)),
                            )
                        }
                    };
                    out.1.push(Handle { path: path.clone(), rect: handle, dir: *dir, parent: area });
                    path.push(false);
                    go(a, first, gap, path, out);
                    path.pop();
                    path.push(true);
                    go(b, second, gap, path, out);
                    path.pop();
                }
            }
        }
        let mut out = (Vec::new(), Vec::new());
        go(self.root(), area, gap, &mut Vec::new(), &mut out);
        out
    }

    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        let mut node = self.root.as_mut().expect("el árbol nunca queda vacío");
        for step in path {
            node = match node {
                Node::Split { a, b, .. } => if *step { b } else { a },
                Node::Leaf { .. } => return,
            };
        }
        if let Node::Split { ratio: current, .. } = node {
            *current = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 600.0))
    }

    #[test]
    fn splitting_places_the_new_leaf_after_the_target() {
        let mut tree = Tree::new(1, "a");
        assert!(tree.split(1, Dir::Horizontal, 2, "b"));
        assert!(tree.split(2, Dir::Vertical, 3, "c"));
        assert_eq!(tree.ids(), vec![1, 2, 3]);
        assert_eq!(tree.get(3), Some(&"c"));
        assert!(!tree.split(99, Dir::Horizontal, 4, "x"));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn removing_a_leaf_gives_its_space_to_the_sibling() {
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Horizontal, 2, ());
        tree.split(2, Dir::Vertical, 3, ());
        assert!(tree.remove(2));
        assert_eq!(tree.ids(), vec![1, 3]);

        let (leaves, handles) = tree.layout(area(), 0.0);
        assert_eq!(handles.len(), 1);
        assert_eq!(leaves.len(), 2);
    }

    #[test]
    fn the_last_leaf_cannot_be_removed() {
        let mut tree = Tree::new(1, ());
        assert!(!tree.remove(1));
        tree.split(1, Dir::Vertical, 2, ());
        assert!(tree.remove(1));
        assert!(!tree.remove(2));
        assert_eq!(tree.ids(), vec![2]);
    }

    #[test]
    fn horizontal_layout_splits_the_width_and_leaves_a_gap() {
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Horizontal, 2, ());
        let (leaves, handles) = tree.layout(area(), 10.0);
        assert_eq!(leaves[0].1.max.x, 495.0);
        assert_eq!(leaves[1].1.min.x, 505.0);
        assert_eq!(leaves[0].1.height(), 600.0);
        assert_eq!(handles[0].rect.center().x, 500.0);
        assert_eq!(handles[0].rect.width(), 10.0);
    }

    #[test]
    fn dragging_a_handle_changes_the_ratio_within_limits() {
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Vertical, 2, ());
        let (_, handles) = tree.layout(area(), 8.0);
        let handle = handles[0].clone();
        assert_eq!(handle.ratio_at(Pos2::new(10.0, 150.0)), 0.25);
        assert_eq!(handle.ratio_at(Pos2::new(10.0, 5.0)), 0.15);
        assert_eq!(handle.ratio_at(Pos2::new(10.0, 599.0)), 0.85);

        tree.set_ratio(&handle.path, 0.25);
        let (leaves, _) = tree.layout(area(), 0.0);
        assert_eq!(leaves[0].1.height(), 150.0);
    }

    /// Izquierda: 1 · derecha arriba: 2 · derecha abajo: 3.
    fn three_panes() -> Vec<(u64, Rect)> {
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Horizontal, 2, ());
        tree.split(2, Dir::Vertical, 3, ());
        tree.layout(area(), 16.0).0
    }

    #[test]
    fn focus_moves_to_the_adjacent_pane_on_that_side() {
        let leaves = three_panes();
        assert_eq!(neighbor(&leaves, 1, Side::Right), Some(2));
        assert_eq!(neighbor(&leaves, 2, Side::Left), Some(1));
        assert_eq!(neighbor(&leaves, 3, Side::Left), Some(1));
        assert_eq!(neighbor(&leaves, 2, Side::Down), Some(3));
        assert_eq!(neighbor(&leaves, 3, Side::Up), Some(2));
    }

    #[test]
    fn focus_stays_put_at_the_edges() {
        let leaves = three_panes();
        assert_eq!(neighbor(&leaves, 1, Side::Left), None);
        assert_eq!(neighbor(&leaves, 1, Side::Up), None);
        assert_eq!(neighbor(&leaves, 1, Side::Down), None);
        assert_eq!(neighbor(&leaves, 2, Side::Up), None);
        assert_eq!(neighbor(&leaves, 3, Side::Right), None);
        assert_eq!(neighbor(&leaves, 99, Side::Right), None);
    }

    #[test]
    fn focus_crosses_between_rows_of_different_widths() {
        // Arriba: 1 y 2 lado a lado. Abajo: 3 ocupa todo el ancho.
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Vertical, 3, ());
        tree.split(1, Dir::Horizontal, 2, ());
        let leaves = tree.layout(area(), 16.0).0;
        assert!(matches!(neighbor(&leaves, 3, Side::Up), Some(1 | 2)));
        assert_eq!(neighbor(&leaves, 1, Side::Down), Some(3));
        assert_eq!(neighbor(&leaves, 2, Side::Down), Some(3));
        assert_eq!(neighbor(&leaves, 1, Side::Right), Some(2));
    }

    #[test]
    fn nested_handles_carry_their_path() {
        let mut tree = Tree::new(1, ());
        tree.split(1, Dir::Horizontal, 2, ());
        tree.split(2, Dir::Vertical, 3, ());
        let (_, handles) = tree.layout(area(), 6.0);
        assert_eq!(handles.len(), 2);
        assert_eq!(handles[0].path, Vec::<bool>::new());
        assert_eq!(handles[1].path, vec![true]);
        tree.set_ratio(&handles[1].path, 0.7);
        let (leaves, _) = tree.layout(area(), 0.0);
        assert_eq!(leaves[1].1.height(), 600.0 * 0.7);
    }
}
