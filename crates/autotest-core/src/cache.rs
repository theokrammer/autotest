use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstHashes {
    pub impl_hash: String,
    pub public_contract_hash: String,
}

impl AstHashes {
    pub fn compute(full_ast_repr: &str, public_signature_repr: &str) -> Self {
        Self {
            impl_hash: format!("sha256:{}", hex_sha256(full_ast_repr)),
            public_contract_hash: format!("contract_sha256:{}", hex_sha256(public_signature_repr)),
        }
    }
}

pub fn hex_sha256(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DependencyGraph {
    /// file -> set of types/files it depends on with expected contract_sha256
    pub direct_deps: BTreeMap<String, BTreeMap<String, String>>,
    /// type/file -> set of files that depend on it (reverse index)
    pub reverse_deps: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_dependencies(&mut self, file: &str, deps: BTreeMap<String, String>) {
        // Clear previous reverse mappings for this file
        if let Some(old_deps) = self.direct_deps.get(file) {
            for dep in old_deps.keys() {
                if let Some(rev) = self.reverse_deps.get_mut(dep) {
                    rev.remove(file);
                }
            }
        }

        for dep in deps.keys() {
            self.reverse_deps
                .entry(dep.clone())
                .or_default()
                .insert(file.to_string());
        }

        self.direct_deps.insert(file.to_string(), deps);
    }

    /// Determines files that must be invalidated when a dependency's contract_sha256 changes
    pub fn find_invalidated_files(
        &self,
        changed_contracts: &HashMap<String, String>,
    ) -> BTreeSet<String> {
        let mut invalidated = BTreeSet::new();
        let mut queue: Vec<String> = Vec::new();

        for (dep_name, new_hash) in changed_contracts {
            if let Some(dependents) = self.reverse_deps.get(dep_name) {
                for dep_file in dependents {
                    if let Some(expected_hash) = self.direct_deps.get(dep_file).and_then(|d| d.get(dep_name)) {
                        if expected_hash != new_hash {
                            if invalidated.insert(dep_file.clone()) {
                                queue.push(dep_file.clone());
                            }
                        }
                    }
                }
            }
        }

        // Transitive cascade
        while let Some(file) = queue.pop() {
            if let Some(dependents) = self.reverse_deps.get(&file) {
                for dep_file in dependents {
                    if invalidated.insert(dep_file.clone()) {
                        queue.push(dep_file.clone());
                    }
                }
            }
        }

        invalidated
    }

    /// Tarjan's Strongly Connected Components algorithm to find all SCCs
    pub fn find_strongly_connected_components(&self) -> Vec<Vec<String>> {
        let mut index = 0;
        let mut stack = Vec::new();
        let mut on_stack = HashSet::new();
        let mut indices = HashMap::new();
        let mut lowlinks = HashMap::new();
        let mut sccs = Vec::new();

        let all_nodes: BTreeSet<String> = self
            .direct_deps
            .keys()
            .cloned()
            .chain(self.reverse_deps.keys().cloned())
            .collect();

        for node in all_nodes {
            if !indices.contains_key(&node) {
                self.tarjan_strongconnect(
                    &node,
                    &mut index,
                    &mut stack,
                    &mut on_stack,
                    &mut indices,
                    &mut lowlinks,
                    &mut sccs,
                );
            }
        }

        sccs
    }

    /// Finds only cyclic components (SCCs with more than 1 node, or 1 self-referential node)
    pub fn find_cycles(&self) -> Vec<Vec<String>> {
        self.find_strongly_connected_components()
            .into_iter()
            .filter(|scc| {
                if scc.len() > 1 {
                    true
                } else if scc.len() == 1 {
                    let node = &scc[0];
                    self.direct_deps
                        .get(node)
                        .map(|deps| deps.contains_key(node))
                        .unwrap_or(false)
                } else {
                    false
                }
            })
            .collect()
    }

    fn tarjan_strongconnect(
        &self,
        v: &str,
        index: &mut usize,
        stack: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
        indices: &mut HashMap<String, usize>,
        lowlinks: &mut HashMap<String, usize>,
        sccs: &mut Vec<Vec<String>>,
    ) {
        indices.insert(v.to_string(), *index);
        lowlinks.insert(v.to_string(), *index);
        *index += 1;
        stack.push(v.to_string());
        on_stack.insert(v.to_string());

        if let Some(neighbors) = self.direct_deps.get(v) {
            for w in neighbors.keys() {
                if !indices.contains_key(w) {
                    self.tarjan_strongconnect(
                        w,
                        index,
                        stack,
                        on_stack,
                        indices,
                        lowlinks,
                        sccs,
                    );
                    let w_low = lowlinks[w];
                    let v_low = lowlinks.get_mut(v).unwrap();
                    *v_low = (*v_low).min(w_low);
                } else if on_stack.contains(w) {
                    let w_idx = indices[w];
                    let v_low = lowlinks.get_mut(v).unwrap();
                    *v_low = (*v_low).min(w_idx);
                }
            }
        }

        if lowlinks[v] == indices[v] {
            let mut component = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack.remove(&w);
                component.push(w.clone());
                if w == v {
                    break;
                }
            }
            component.sort();
            sccs.push(component);
        }
    }
}
