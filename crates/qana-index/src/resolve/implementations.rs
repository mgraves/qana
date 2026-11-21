//! Where a declaration is implemented: what Go to Implementation lists.
//!
//! - **A trait**: its implementors, the impl blocks whose trait resolves to
//!   it (a blanket impl among them), and the declarations whose derives
//!   name it, which implement it with no impl block written.
//! - **A trait's item** (a method, an associated type or constant): the
//!   items of the same name and kind of its implementors' blocks, and the
//!   declarations deriving the trait, whose derive writes the item.
//! - **A type**: its impl blocks, inherent and of a trait (an alias's,
//!   those of the type it stands for), and the traits it derives.
//! - **An impl block's item that implements a trait's item**: itself.
//!
//! Anything else (a free function, an inherent method, a field, a module)
//! is implemented nowhere but where it is declared: none.
//!
//! Candidates are found by name, the impl blocks by the last name of their
//! trait's path and the derivers by each derive's, through imports that
//! rename them; each is then resolved, so only those that name the trait
//! are kept.

use std::path::{Path as FilePath, PathBuf};

use super::types::{CLASS, STRUCT};
use super::Resolver;

/// Where a declaration is implemented (see [`Resolver::implementations`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Implementation {
    /// An impl block: its file, and its index into its summary's impl
    /// blocks ([`Offers::impls`](crate::Offers::impls)).
    Block { file: PathBuf, block: u32 },
    /// An impl block's item that implements a trait's item: its file, and
    /// its index into its summary's declarations.
    Item { file: PathBuf, declaration: u32 },
    /// A declaration that derives a trait (`#[derive(Clone)]`), which
    /// implements it with no impl block written: its file and index, and
    /// the trait it derives, by its file and index.
    Derived { file: PathBuf, declaration: u32, derives: (PathBuf, u32) },
}

impl Resolver {
    /// Where the `declaration`th declaration of `file` is implemented, in
    /// files' order. See the [module docs](self).
    pub fn implementations(&self, file: &FilePath, declaration: u32) -> Vec<Implementation> {
        let Some(&id) = self.ids.get(file) else {
            return Vec::new();
        };
        let declarations = self.summaries[id as usize].offers().declarations();
        let Some(declared) = declarations.get(declaration as usize) else {
            return Vec::new();
        };
        let d = (id, declaration);
        let mut found = Vec::new();
        if let Some(block) = declared.impl_block {
            // An item of an impl block of a trait implements one itself.
            if !self.is_inherent((id, block)) {
                found.push(Implementation::Item { file: file.to_path_buf(), declaration });
            }
            return found;
        }
        match self.owner_of(d).filter(|&owner| self.declaration(owner).kind == CLASS) {
            // A trait's item: its implementors' items like it.
            Some(t) => {
                for block in self.implementors(t) {
                    let items = self.summaries[block.0 as usize].offers().declarations();
                    for &i in self.items_of_block(block).iter() {
                        let item = &items[i as usize];
                        if item.name == declared.name && item.kind == declared.kind {
                            found.push(Implementation::Item {
                                file: self.files[block.0 as usize].clone(),
                                declaration: i,
                            });
                        }
                    }
                }
                found.extend(self.derived_from(t));
            }
            None if declared.kind == CLASS => {
                for block in self.implementors(d) {
                    found.push(Implementation::Block {
                        file: self.files[block.0 as usize].clone(),
                        block: block.1,
                    });
                }
                found.extend(self.derived_from(d));
            }
            None if declared.kind == STRUCT => {
                for ty in self.unaliased(d) {
                    for &block in self.impls_of(ty.0, ty.1).iter() {
                        found.push(Implementation::Block {
                            file: self.files[block.0 as usize].clone(),
                            block: block.1,
                        });
                    }
                    for t in self.derived(ty) {
                        found.push(Implementation::Derived {
                            file: self.files[ty.0 as usize].clone(),
                            declaration: ty.1,
                            derives: (self.files[t.0 as usize].clone(), t.1),
                        });
                    }
                }
            }
            None => {}
        }
        found.sort();
        found.dedup();
        found
    }

    /// The traits each derive of the `declaration`th declaration of `file`
    /// implements, in [`Declaration::derives`](crate::Declaration::derives)'
    /// order, each by its file and index: what the derive's path names, or
    /// the built-in derive's, by its name (see
    /// [`CrateInfo::derive_traits`](crate::CrateInfo::derive_traits)).
    pub fn derived_traits(&self, file: &FilePath, declaration: u32) -> Vec<Vec<(PathBuf, u32)>> {
        let Some(&id) = self.ids.get(file) else {
            return Vec::new();
        };
        let declarations = self.summaries[id as usize].offers().declarations();
        let Some(declared) = declarations.get(declaration as usize) else {
            return Vec::new();
        };
        declared
            .derives
            .iter()
            .map(|path| {
                self.derive_implements((id, declaration), path)
                    .into_iter()
                    .map(|t| (self.files[t.0 as usize].clone(), t.1))
                    .collect()
            })
            .collect()
    }

    /// Whether the `declaration`th declaration of `file` is implemented
    /// anywhere: whether [`implementations`](Self::implementations) has an
    /// answer, found as far as the first.
    pub fn is_implemented(&self, file: &FilePath, declaration: u32) -> bool {
        let Some(&id) = self.ids.get(file) else {
            return false;
        };
        let declarations = self.summaries[id as usize].offers().declarations();
        let Some(declared) = declarations.get(declaration as usize) else {
            return false;
        };
        let d = (id, declaration);
        if let Some(block) = declared.impl_block {
            return !self.is_inherent((id, block));
        }
        match self.owner_of(d).filter(|&owner| self.declaration(owner).kind == CLASS) {
            Some(t) => {
                self.implementor_candidates(t).into_iter().any(|block| {
                    self.impl_traits(block).contains(&t) && {
                        let items = self.summaries[block.0 as usize].offers().declarations();
                        self.items_of_block(block).iter().any(|&i| {
                            let item = &items[i as usize];
                            item.name == declared.name && item.kind == declared.kind
                        })
                    }
                }) || self.deriver_candidates(t).into_iter().any(|d| self.derived(d).contains(&t))
            }
            None if declared.kind == CLASS => {
                self.implementor_candidates(d)
                    .into_iter()
                    .any(|block| self.impl_traits(block).contains(&d))
                    || self.deriver_candidates(d).into_iter().any(|x| self.derived(x).contains(&d))
            }
            None if declared.kind == STRUCT => self
                .unaliased(d)
                .into_iter()
                .any(|ty| !self.impls_of(ty.0, ty.1).is_empty() || !self.derived(ty).is_empty()),
            None => false,
        }
    }

    /// The impl blocks of trait `t`: those whose trait's path, read where
    /// each block is, resolves to it.
    fn implementors(&self, t: (u32, u32)) -> Vec<(u32, u32)> {
        let mut candidates = self.implementor_candidates(t);
        candidates.retain(|&block| self.impl_traits(block).contains(&t));
        candidates
    }

    /// The impl blocks that may be of trait `t`, by the last name of their
    /// trait's path, through imports that rename it.
    fn implementor_candidates(&self, t: (u32, u32)) -> Vec<(u32, u32)> {
        let names = self.impl_names();
        let mut candidates: Vec<(u32, u32)> = self
            .called(&self.declaration(t).name)
            .iter()
            .filter_map(|name| names.by_trait.get(name))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }

    /// The declarations that may derive trait `t`, by the last name of
    /// each derive's path, through imports that rename it.
    fn deriver_candidates(&self, t: (u32, u32)) -> Vec<(u32, u32)> {
        let names = self.impl_names();
        let mut candidates: Vec<(u32, u32)> = self
            .called(&self.declaration(t).name)
            .iter()
            .filter_map(|name| names.derivers.get(name))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }

    /// The declarations that derive trait `t`, each as an implementation.
    fn derived_from(&self, t: (u32, u32)) -> Vec<Implementation> {
        self.deriver_candidates(t)
            .into_iter()
            .filter(|&d| self.derived(d).contains(&t))
            .map(|d| Implementation::Derived {
                file: self.files[d.0 as usize].clone(),
                declaration: d.1,
                derives: (self.files[t.0 as usize].clone(), t.1),
            })
            .collect()
    }
}
