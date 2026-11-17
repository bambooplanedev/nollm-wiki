//! C++ declaration pairing: matches header declarations to their implementations, per file pair and per project.

use super::arity;
use crate::formats::code::{ext_of, is_cpp, is_cpp_fragment, module_stem, source_rank, CodeUnit};
use crate::model::{CodeItem, CodeItemKind, PairRole};
use std::collections::BTreeMap;

type Key<'a> = (Option<&'a str>, &'a str, Option<usize>);

/// The `(owner, name, arity)` keys that occur exactly once among the
/// `Fn`/`Method` items given, each with the location `L` it was given with.
/// Overloads with the same parameter count share a key, so they drop out here.
fn unique_keys<'a, L>(items: impl IntoIterator<Item = (L, &'a CodeItem)>) -> BTreeMap<Key<'a>, L> {
    let mut seen: BTreeMap<Key<'a>, Option<L>> = BTreeMap::new();
    for (at, it) in items {
        if matches!(it.kind, CodeItemKind::Fn | CodeItemKind::Method) {
            let key = (
                it.owner.as_deref(),
                it.name.as_str(),
                arity(&it.signature, &it.name),
            );
            seen.entry(key)
                .and_modify(|v| *v = None)
                .or_insert(Some(at));
        }
    }
    seen.into_iter()
        .filter_map(|(k, v)| v.map(|at| (k, at)))
        .collect()
}

/// The header and the implementation file of a group of files: its only
/// `.cpp` and its only lowest-rank header, when both are unique.
fn header_and_impl<'a, U: CodeUnit>(
    entities: &BTreeMap<String, U>,
    ids: &[&'a str],
) -> Option<(&'a str, &'a str)> {
    let rank = |id: &str| source_rank(entities[id].path());
    let mut impls: Vec<&str> = ids.iter().copied().filter(|id| rank(id) == 2).collect();
    // A fragment is an implementation only when no real implementation file is there.
    if impls
        .iter()
        .any(|id| !is_cpp_fragment(entities[*id].path()))
    {
        impls.retain(|id| !is_cpp_fragment(entities[*id].path()));
    }
    let min = ids.iter().map(|id| rank(id)).min().filter(|r| *r < 2)?;
    let headers: Vec<&str> = ids.iter().copied().filter(|id| rank(id) == min).collect();
    match (impls.as_slice(), headers.as_slice()) {
        ([imp], [hdr]) => Some((*hdr, *imp)),
        _ => None,
    }
}

/// Pairs a header's declarations with their definitions: the doc usually
/// sits on the declaration, so the definition inherits it, and each side's
/// `twin` names the other's lines. Two steps. Same stem first: a group with
/// exactly one implementation file and a unique lowest-rank header pairs (a
/// same-stem group that is not one pairs within each directory), on keys
/// unique within that file pair, a `Declaration` header item with an
/// `OutOfLineDefinition` implementation item (`pair_role`). Then the whole
/// project (`pair_project`):
/// keys unique among all header items and among all implementation items.
/// A wrong pairing is worse than none, so overloads with the same parameter
/// count and keys with two declarations or two definitions stay unpaired.
pub fn pair_declarations<U: CodeUnit>(entities: &mut BTreeMap<String, U>) {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, e) in entities.iter() {
        let sp = e.path();
        if is_cpp(&ext_of(sp)) {
            groups.entry(module_stem(sp)).or_default().push(id);
        }
    }
    let mut pairs: Vec<(String, usize, String, usize)> = Vec::new();
    for ids in groups.values() {
        let found = header_and_impl(entities, ids).map_or_else(
            || {
                // Same-stem files in several directories (`src/exchange/x.cpp`,
                // `src/execution/x.cpp`) pair within each directory.
                let mut dirs: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
                for &id in ids {
                    let sp = entities[id].path();
                    dirs.entry(sp.rsplit_once('/').map_or("", |(d, _)| d))
                        .or_default()
                        .push(id);
                }
                dirs.values()
                    .filter_map(|d| header_and_impl(entities, d))
                    .collect()
            },
            |p| vec![p],
        );
        for (hdr, imp) in found {
            let (h, c) = (entities[hdr].items(), entities[imp].items());
            let cu = unique_keys(c.iter().enumerate());
            for (key, hi) in unique_keys(h.iter().enumerate()) {
                let Some(&ci) = cu.get(&key) else { continue };
                if h[hi].pair_role == PairRole::Declaration
                    && c[ci].pair_role == PairRole::OutOfLineDefinition
                {
                    pairs.push((hdr.to_string(), hi, imp.to_string(), ci));
                }
            }
        }
    }
    pair_project(entities, &mut pairs);
    for (hid, hi, cid, ci) in pairs {
        let span = |e: &U, i: usize| e.items()[i].span(e.path());
        let (h_span, c_span) = (span(&entities[&hid], hi), span(&entities[&cid], ci));
        let h_doc = &entities[&hid].items()[hi];
        let (doc, doc_text) = (h_doc.doc.clone(), h_doc.doc_text.clone());
        let h_item = &mut entities.get_mut(&hid).unwrap().items_mut()[hi];
        h_item.twin = Some(c_span);
        let c_item = &mut entities.get_mut(&cid).unwrap().items_mut()[ci];
        c_item.twin = Some(h_span);
        if c_item.doc.is_none() {
            c_item.doc = doc;
            c_item.doc_text = doc_text;
        }
    }
}

/// The project step of `pair_declarations`: appends every key
/// `(owner, name, arity)` that occurs exactly once among the `Fn`/`Method`
/// items of all C++ headers and exactly once among those of all
/// implementation files. Only a `Declaration` header item and an
/// `OutOfLineDefinition` implementation item pair (`pair_role`), but
/// uniqueness counts every item, whatever its role and paired or not, so an
/// item the same-stem step paired blocks any other partner. There is no
/// "already paired" set: a key unique project-wide is unique inside its file
/// pair too, so this step can only re-find a same-stem pair, and applying a
/// pair twice leaves `twin` and `doc` as after once (the definition's `doc`
/// is filled only while empty).
fn pair_project<U: CodeUnit>(
    entities: &BTreeMap<String, U>,
    pairs: &mut Vec<(String, usize, String, usize)>,
) {
    let unique = |header: bool| {
        unique_keys(
            entities
                .iter()
                .filter(|(_, e)| is_cpp(&ext_of(e.path())) && (source_rank(e.path()) < 2) == header)
                .flat_map(|(id, e)| {
                    e.items()
                        .iter()
                        .enumerate()
                        .map(move |(i, it)| ((id.as_str(), i), it))
                }),
        )
    };
    let role = |id: &str, i: usize| entities[id].items()[i].pair_role;
    let impls = unique(false);
    for (key, h) in unique(true) {
        if let ((hid, hi), Some(&(cid, ci))) = (h, impls.get(&key)) {
            if role(hid, hi) == PairRole::Declaration
                && role(cid, ci) == PairRole::OutOfLineDefinition
            {
                pairs.push((hid.to_string(), hi, cid.to_string(), ci));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodeFile;

    fn len_twin(
        files: [(&str, &str); 3],
        hdr: &str,
        imp: &str,
    ) -> (Option<String>, Option<String>) {
        let mut files: BTreeMap<String, CodeFile> = files
            .map(|(p, s)| (p.to_string(), crate::extract(p, s)))
            .into();
        pair_declarations(&mut files);
        let len = |p: &str| files[p].items.iter().find(|i| i.name == "len").unwrap();
        (len(hdr).twin.clone(), len(imp).twin.clone())
    }

    #[test]
    fn a_fragment_is_the_implementation_when_it_is_the_only_companion() {
        let h = "class Vec {\n  int len() const;\n};\n";
        let d = "int Vec::len() const { return 0; }\n";
        let (ht, it) = len_twin(
            [("lib/vec.h", h), ("lib/vec.inl", d), ("lib/other.cpp", d)],
            "lib/vec.h",
            "lib/vec.inl",
        );
        assert_eq!(ht.as_deref(), Some("lib/vec.inl:1-1"));
        assert_eq!(it.as_deref(), Some("lib/vec.h:2-2"));
    }

    #[test]
    fn a_real_implementation_wins_over_two_fragments() {
        let h = "class Vec {\n  int len() const;\n};\n";
        let d = "int Vec::len() const { return 0; }\n";
        let mut files: BTreeMap<String, CodeFile> = [
            ("lib/vec.h", h),
            ("lib/vec.cpp", d),
            ("lib/vec.inl", d),
            ("lib/vec.tcc", d),
        ]
        .map(|(p, s)| (p.to_string(), crate::extract(p, s)))
        .into();
        pair_declarations(&mut files);
        let hdr = files["lib/vec.h"]
            .items
            .iter()
            .find(|i| i.name == "len")
            .unwrap();
        assert_eq!(hdr.twin.as_deref(), Some("lib/vec.cpp:1-1"));
    }
}
