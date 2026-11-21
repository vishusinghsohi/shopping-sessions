use std::collections::HashMap;

#[derive(Default)]
pub struct IdentityMap {
    ids: HashMap<String, usize>,
    parent: Vec<usize>,
    size: Vec<usize>,
    minimum: Vec<String>,
}

impl IdentityMap {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_or_insert(&mut self, id: &str) -> usize {
        if let Some(&index) = self.ids.get(id) {
            return index;
        }

        let index = self.parent.len();

        self.ids.insert(id.to_owned(), index);
        self.parent.push(index);
        self.size.push(1);
        self.minimum.push(id.to_owned());

        index
    }

    fn find(&mut self, index: usize) -> usize {
        let mut root = index;

        while self.parent[root] != root {
            root = self.parent[root];
        }

        let mut current = index;

        while self.parent[current] != current {
            let next = self.parent[current];
            self.parent[current] = root;
            current = next;
        }

        root
    }

    pub fn union(&mut self, a: &str, b: &str) {
        // Empty IDs cannot form valid links.
        if a.is_empty() || b.is_empty() {
            return;
        }

        let a_index = self.get_or_insert(a);
        let b_index = self.get_or_insert(b);

        let mut root_a = self.find(a_index);
        let mut root_b = self.find(b_index);

        if root_a == root_b {
            return;
        }

        // Union by size: keep the larger component as root.
        if self.size[root_a] < self.size[root_b] {
            std::mem::swap(&mut root_a, &mut root_b);
        }

        self.parent[root_b] = root_a;
        self.size[root_a] += self.size[root_b];

        // Select the smallest UTF-8 byte-wise ID.
        if self.minimum[root_b].as_bytes() < self.minimum[root_a].as_bytes() {
            self.minimum[root_a] = self.minimum[root_b].clone();
        }
    }

    pub fn canonical(&mut self, id: &str) -> String {
        let Some(&index) = self.ids.get(id) else {
            // Unlinked IDs remain unchanged.
            return id.to_owned();
        };

        let root = self.find(index);
        self.minimum[root].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_transitive_links() {
        let mut identities = IdentityMap::new();

        identities.union("u3", "u9");
        identities.union("u9", "u2");
        identities.union("U7", "u9");

        assert_eq!(identities.canonical("u3"), "U7");
        assert_eq!(identities.canonical("u2"), "U7");
        assert_eq!(identities.canonical("u9"), "U7");
    }

    #[test]
    fn preserves_case_and_spaces() {
        let mut identities = IdentityMap::new();

        identities.union("u1", "u2");

        assert_eq!(identities.canonical("u2"), "u1");
        assert_eq!(identities.canonical("U1"), "U1");
        assert_eq!(identities.canonical(" u1"), " u1");
    }

    #[test]
    fn handles_self_links_and_cycles() {
        let mut identities = IdentityMap::new();

        identities.union("b", "b");
        identities.union("b", "a");
        identities.union("a", "b");

        assert_eq!(identities.canonical("b"), "a");
    }

    #[test]
    fn ignores_empty_link_sides() {
        let mut identities = IdentityMap::new();

        identities.union("u1", "");
        identities.union("", "u2");

        assert_eq!(identities.canonical("u1"), "u1");
        assert_eq!(identities.canonical("u2"), "u2");
    }
}
