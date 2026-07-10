//! Curated ledger-ADT method surfaces for completion + hover.
//!
//! The ledger ADTs (Counter, Cell, Map, Set, List, MerkleTree,
//! HistoricMerkleTree, Kernel) are compiler builtins invoked with method
//! syntax (`counter.increment(1)`), not Compact circuits — so they cannot be
//! expressed as a `.compact` stub. This table is curated from the compiler
//! source at the pinned tag and embedded as JSON.
//!
//! Provenance: `LFDT-Minokawa/compact compiler/midnight-ledger.ss`
//! (`declare-ledger-adt` forms) + `standard-library.compact` (the `kernel`
//! field), language 0.23 / compiler 0.31.0 / ledger 8.0.2. In-circuit methods
//! only; js-only and coin-conditional methods are omitted (see the asset's
//! `_meta.note`). Refresh: re-read `midnight-ledger.ss` at the target compiler
//! tag and re-confirm each method with `compact compile --skip-zk` probes;
//! bump the asset filename's language version and this doc-comment together.

use compactp_ast::AstNode;
use std::collections::BTreeMap;

/// One in-circuit method of a ledger ADT.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LedgerMethod {
    pub name: String,
    pub sig: String,
    pub doc: String,
}

const LEDGER_ADTS_JSON: &str = include_str!("../assets/ledger_adts_0_23.json");

/// The parsed ADT → methods table. The `_meta` object in the JSON is ignored.
pub(crate) struct LedgerAdtTable {
    by_adt: BTreeMap<String, Vec<LedgerMethod>>,
}

impl LedgerAdtTable {
    pub(crate) fn load() -> Self {
        // The asset is embedded and covered by a parse test; a malformed asset
        // is a build-time authoring error, but never panic the server — fall
        // back to an empty table so completion/hover simply offer nothing.
        let raw: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(LEDGER_ADTS_JSON).unwrap_or_default();
        let mut by_adt = BTreeMap::new();
        for (adt, value) in raw {
            if adt == "_meta" {
                continue;
            }
            if let Ok(methods) = serde_json::from_value::<Vec<LedgerMethod>>(value) {
                by_adt.insert(adt, methods);
            }
        }
        Self { by_adt }
    }

    pub(crate) fn methods(&self, adt: &str) -> &[LedgerMethod] {
        self.by_adt.get(adt).map(Vec::as_slice).unwrap_or(&[])
    }
}

impl crate::AnalysisHost {
    /// In-circuit method surface for a ledger ADT head, or `&[]` if unknown.
    pub fn ledger_adt_methods(&self, adt: &str) -> &[LedgerMethod] {
        self.ledger_adts_table().methods(adt)
    }

    /// If `def` is a ledger field, the ADT key to look its methods up under:
    /// the declared type head when it is a known ADT
    /// (Counter/Map/Set/List/MerkleTree/HistoricMerkleTree/Kernel), otherwise
    /// `"Cell"` — a plain-typed ledger field is implicitly a Cell (F7).
    pub fn ledger_field_adt(&mut self, def: &crate::Definition) -> Option<String> {
        let crate::Definition::Item { file, index } = def else {
            return None;
        };
        let analysis = self.analyze(*file)?;
        let sym = analysis.item_tree.symbols.get(*index as usize)?;
        if sym.kind != crate::SymbolKind::Ledger {
            return None;
        }
        let name_range = sym.name_range;
        let root = crate::SyntaxNode::new_root(analysis.green.clone());
        // Find the LEDGER_DECL whose name token matches, read its type head.
        let ledger = root
            .descendants()
            .filter_map(compactp_ast::LedgerDecl::cast)
            .find(|d| d.name().is_some_and(|t| t.text_range() == name_range))?;
        let head = match ledger.ty()? {
            compactp_ast::Type::Ref(r) => r.name()?.text().to_string(),
            _ => return Some("Cell".to_string()), // builtin scalar type → Cell
        };
        const KNOWN: &[&str] = &[
            "Counter",
            "Cell",
            "Map",
            "Set",
            "List",
            "MerkleTree",
            "HistoricMerkleTree",
            "Kernel",
        ];
        Some(if KNOWN.contains(&head.as_str()) {
            head
        } else {
            "Cell".to_string() // user/struct-typed field → implicit Cell
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_covers_all_adts() {
        let table = LedgerAdtTable::load();
        for adt in [
            "Counter",
            "Cell",
            "Map",
            "Set",
            "List",
            "MerkleTree",
            "HistoricMerkleTree",
            "Kernel",
        ] {
            assert!(!table.methods(adt).is_empty(), "missing ADT: {adt}");
        }
        // Probe-confirmed non-obvious facts (F7).
        let inc = table
            .methods("Counter")
            .iter()
            .find(|m| m.name == "increment")
            .unwrap();
        assert_eq!(inc.sig, "increment(amount: Uint<16>): []");
        assert!(table.methods("nonsense").is_empty());
        // js-only / coin methods are intentionally absent.
        assert!(!table.methods("MerkleTree").iter().any(|m| m.name == "root"));
        assert!(!table.methods("Map").iter().any(|m| m.name == "insertCoin"));
    }

    #[test]
    fn ledger_field_adt_maps_head_and_implicit_cell() {
        let mut host = crate::AnalysisHost::new();
        let file = host
            .vfs_mut()
            .file_id(std::path::Path::new("/t/main.compact"));
        host.vfs_mut().set_overlay(
            file,
            "export ledger cnt: Counter;\nexport ledger bal: Uint<64>;\n".to_string(),
            1,
        );
        let tree = host.analyze(file).unwrap().item_tree.clone();
        let cnt = tree.symbols.iter().position(|s| s.name == "cnt").unwrap();
        let bal = tree.symbols.iter().position(|s| s.name == "bal").unwrap();
        assert_eq!(
            host.ledger_field_adt(&crate::Definition::Item {
                file,
                index: cnt as u32
            }),
            Some("Counter".to_string())
        );
        // Plain-typed ledger field is an implicit Cell.
        assert_eq!(
            host.ledger_field_adt(&crate::Definition::Item {
                file,
                index: bal as u32
            }),
            Some("Cell".to_string())
        );
    }
}
