//! Static analysis for def-use relations.

use std::{
    collections::HashMap,
    ops::{Deref, Range},
    sync::Arc,
};

use log::info;
use reflexo::path::unix_slash;
pub use reflexo::vector::ir::DefId;
use serde::Serialize;
use typst::syntax::FileId as TypstFileId;
use typst::syntax::Source;

use super::SearchCtx;
use crate::syntax::{
    find_source_by_import_path, IdentRef, LexicalHierarchy, LexicalKind, LexicalVarKind, ModSrc,
};
use crate::{adt::snapshot_map::SnapshotMap, syntax::LexicalModKind};

/// The type namespace of def-use relations
///
/// The symbols from different namespaces are not visible to each other.
enum Ns {
    /// Def-use for labels
    Label,
    /// Def-use for values
    Value,
}

/// A flat and transient reference to some symbol in a source file.
///
/// See [`IdentRef`] for definition of a "transient" reference.
#[derive(Serialize, Clone)]
pub struct IdentDef {
    /// The name of the symbol.
    pub name: String,
    /// The kind of the symbol.
    pub kind: LexicalKind,
    /// The byte range of the symbol in the source file.
    pub range: Range<usize>,
}

type ExternalRefMap = HashMap<(TypstFileId, Option<String>), Vec<(Option<DefId>, IdentRef)>>;

/// The def-use information of a source file.
#[derive(Default)]
pub struct DefUseInfo {
    ident_defs: indexmap::IndexMap<(TypstFileId, IdentRef), IdentDef>,
    external_refs: ExternalRefMap,
    ident_refs: HashMap<IdentRef, DefId>,
    undefined_refs: Vec<IdentRef>,
    exports_refs: Vec<DefId>,
    exports_defs: HashMap<String, DefId>,
}

impl DefUseInfo {
    /// Get the estimated memory usage of the def-use information.
    pub fn estimated_memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.ident_defs.capacity()
                * (std::mem::size_of::<IdentDef>() + std::mem::size_of::<IdentRef>() + 32)
            + self.external_refs.capacity()
                * (std::mem::size_of::<(TypstFileId, Option<String>)>()
                    + std::mem::size_of::<Vec<(Option<DefId>, IdentRef)>>()
                    + 32)
            + self.ident_refs.capacity()
                * (std::mem::size_of::<IdentRef>() + std::mem::size_of::<DefId>() + 32)
            + (self.undefined_refs.capacity() * std::mem::size_of::<IdentRef>() + 32)
            + (self.exports_refs.capacity() * std::mem::size_of::<DefId>() + 32)
            + self.exports_defs.capacity()
                * (std::mem::size_of::<String>() + std::mem::size_of::<DefId>() + 32)
    }

    /// Get the definition id of a symbol by its name reference.
    pub fn get_ref(&self, ident: &IdentRef) -> Option<DefId> {
        self.ident_refs.get(ident).copied()
    }

    /// Get the definition of a symbol by its unique id.
    pub fn get_def_by_id(&self, id: DefId) -> Option<(TypstFileId, &IdentDef)> {
        let ((fid, _), def) = self.ident_defs.get_index(id.0 as usize)?;
        Some((*fid, def))
    }

    /// Get the definition of a symbol by its name reference.
    pub fn get_def(&self, fid: TypstFileId, ident: &IdentRef) -> Option<(DefId, &IdentDef)> {
        let (id, _, def) = self.ident_defs.get_full(&(fid, ident.clone()))?;
        Some((DefId(id as u64), def))
    }

    /// Get the references of a symbol by its unique id.
    pub fn get_refs(&self, id: DefId) -> impl Iterator<Item = &IdentRef> {
        self.ident_refs
            .iter()
            .filter_map(move |(k, v)| if *v == id { Some(k) } else { None })
    }

    /// Get external references of a symbol by its name reference.
    pub fn get_external_refs(
        &self,
        ext_id: TypstFileId,
        ext_name: Option<String>,
    ) -> impl Iterator<Item = &(Option<DefId>, IdentRef)> {
        self.external_refs
            .get(&(ext_id, ext_name))
            .into_iter()
            .flatten()
    }

    /// Check if a symbol is exported.
    pub fn is_exported(&self, id: DefId) -> bool {
        self.exports_refs.contains(&id)
    }
}

pub(super) fn get_def_use_inner(ctx: &mut SearchCtx, source: Source) -> Option<Arc<DefUseInfo>> {
    let current_id = source.id();
    ctx.ctx.get_mut(current_id);
    let c = ctx.ctx.get(current_id).unwrap();

    if let Some(info) = c.def_use() {
        return Some(info);
    }

    if !ctx.searched.insert(current_id) {
        return None;
    }

    let e = ctx.ctx.def_use_lexical_hierarchy(source)?;

    let mut collector = DefUseCollector {
        ctx,
        info: DefUseInfo::default(),
        id_scope: SnapshotMap::default(),
        label_scope: SnapshotMap::default(),

        current_id,
        ext_src: None,
    };

    collector.scan(&e);
    collector.calc_exports();
    let res = Some(Arc::new(collector.info));

    let c = ctx.ctx.get(current_id).unwrap();
    // todo: cyclic import cause no any information
    c.compute_def_use(|| res.clone());
    res
}

struct DefUseCollector<'a, 'b, 'w> {
    ctx: &'a mut SearchCtx<'b, 'w>,
    info: DefUseInfo,
    label_scope: SnapshotMap<String, DefId>,
    id_scope: SnapshotMap<String, DefId>,

    current_id: TypstFileId,
    ext_src: Option<Source>,
}

impl<'a, 'b, 'w> DefUseCollector<'a, 'b, 'w> {
    fn enter<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let id_snap = self.id_scope.snapshot();
        let res = f(self);
        self.id_scope.rollback_to(id_snap);
        res
    }

    fn calc_exports(&mut self) {
        self.info.exports_refs = self.id_scope.values().copied().collect();
        self.info.exports_defs = self
            .id_scope
            .entries()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
    }

    fn import_name(&mut self, name: &str) -> Option<()> {
        let source = self.ext_src.as_ref()?;

        log::debug!("import for def use: {:?}, name: {name}", source.id());
        let (_, external_info) =
            Some(source.id()).zip(get_def_use_inner(self.ctx, source.clone()))?;

        let ext_id = external_info.exports_defs.get(name)?;
        self.import_from(&external_info, *ext_id);

        Some(())
    }

    fn import_from(&mut self, external_info: &DefUseInfo, v: DefId) {
        // Use FileId in ident_defs map should lose stacked import
        // information, but it is currently
        // not a problem.
        let ((ext_id, _), ext_sym) = external_info.ident_defs.get_index(v.0 as usize).unwrap();

        let name = ext_sym.name.clone();

        let ext_ref = IdentRef {
            name: name.clone(),
            range: ext_sym.range.clone(),
        };

        let (id, ..) = self
            .info
            .ident_defs
            .insert_full((*ext_id, ext_ref), ext_sym.clone());

        let id = DefId(id as u64);
        self.id_scope.insert(name, id);
    }

    fn scan(&mut self, e: &'a [LexicalHierarchy]) -> Option<()> {
        for e in e {
            match &e.info.kind {
                LexicalKind::Heading(..) => unreachable!(),
                LexicalKind::Mod(LexicalModKind::PathInclude) => {}
                LexicalKind::Var(LexicalVarKind::Label) => {
                    self.insert(Ns::Label, e);
                }
                LexicalKind::Var(LexicalVarKind::LabelRef) => self.insert_ref(Ns::Label, e),
                LexicalKind::Var(LexicalVarKind::Function)
                | LexicalKind::Var(LexicalVarKind::Variable) => {
                    self.insert(Ns::Value, e);
                }
                LexicalKind::Mod(LexicalModKind::PathVar)
                | LexicalKind::Mod(LexicalModKind::ModuleAlias) => self.insert_module(Ns::Value, e),
                LexicalKind::Mod(LexicalModKind::Ident) => match self.import_name(&e.info.name) {
                    Some(()) => {
                        self.insert_ref(Ns::Value, e);
                    }
                    None => {
                        let def_id = self.insert(Ns::Value, e);
                        self.insert_extern(e.info.name.clone(), e.info.range.clone(), Some(def_id));
                    }
                },
                LexicalKind::Mod(LexicalModKind::Alias { target }) => {
                    match self.import_name(&target.name) {
                        Some(()) => {
                            self.insert_ident_ref(
                                Ns::Value,
                                IdentRef {
                                    name: target.name.clone(),
                                    range: target.range.clone(),
                                },
                            );
                            self.insert(Ns::Value, e);
                        }
                        None => {
                            let def_id = self.insert(Ns::Value, e);
                            self.insert_extern(
                                target.name.clone(),
                                target.range.clone(),
                                Some(def_id),
                            );
                        }
                    }
                }
                LexicalKind::Var(LexicalVarKind::ValRef) => self.insert_ref(Ns::Value, e),
                LexicalKind::Block => {
                    if let Some(e) = &e.children {
                        self.enter(|this| this.scan(e.as_slice()))?;
                    }
                }
                LexicalKind::Mod(LexicalModKind::Module(p)) => {
                    match p {
                        ModSrc::Expr(_) => {}
                        ModSrc::Path(p) => {
                            let src = find_source_by_import_path(
                                self.ctx.ctx.world(),
                                self.current_id,
                                p.deref(),
                            );
                            self.ext_src = src;
                        }
                    }

                    // todo: process import star
                    if let Some(e) = &e.children {
                        self.scan(e.as_slice())?;
                    }

                    self.ext_src = None;
                }
                LexicalKind::Mod(LexicalModKind::Star) => {
                    if let Some(source) = &self.ext_src {
                        info!("diving source for def use: {:?}", source.id());
                        let (_, external_info) =
                            Some(source.id()).zip(get_def_use_inner(self.ctx, source.clone()))?;

                        for ext_id in &external_info.exports_refs {
                            self.import_from(&external_info, *ext_id);
                        }
                    }
                }
            }
        }

        Some(())
    }

    fn insert_module(&mut self, label: Ns, e: &LexicalHierarchy) {
        self.insert(label, e);
        if let Some(src) = &self.ext_src {
            self.info.external_refs.insert(
                (src.id(), None),
                vec![(
                    None,
                    IdentRef {
                        name: e.info.name.clone(),
                        range: e.info.range.clone(),
                    },
                )],
            );
        }
    }

    fn insert_extern(&mut self, name: String, range: Range<usize>, redefine_id: Option<DefId>) {
        if let Some(src) = &self.ext_src {
            self.info.external_refs.insert(
                (src.id(), Some(name.clone())),
                vec![(redefine_id, IdentRef { name, range })],
            );
        }
    }

    fn insert(&mut self, label: Ns, e: &LexicalHierarchy) -> DefId {
        let snap = match label {
            Ns::Label => &mut self.label_scope,
            Ns::Value => &mut self.id_scope,
        };

        let id_ref = IdentRef {
            name: e.info.name.clone(),
            range: e.info.range.clone(),
        };
        let (id, ..) = self.info.ident_defs.insert_full(
            (self.current_id, id_ref.clone()),
            IdentDef {
                name: e.info.name.clone(),
                kind: e.info.kind.clone(),
                range: e.info.range.clone(),
            },
        );

        let id = DefId(id as u64);
        snap.insert(e.info.name.clone(), id);
        id
    }

    fn insert_ident_ref(&mut self, label: Ns, id_ref: IdentRef) {
        let snap = match label {
            Ns::Label => &mut self.label_scope,
            Ns::Value => &mut self.id_scope,
        };

        match snap.get(&id_ref.name) {
            Some(id) => {
                self.info.ident_refs.insert(id_ref, *id);
            }
            None => {
                self.info.undefined_refs.push(id_ref);
            }
        }
    }

    fn insert_ref(&mut self, label: Ns, e: &LexicalHierarchy) {
        self.insert_ident_ref(
            label,
            IdentRef {
                name: e.info.name.clone(),
                range: e.info.range.clone(),
            },
        );
    }
}

/// A snapshot of the def-use information for testing.
pub struct DefUseSnapshot<'a>(pub &'a DefUseInfo);

impl<'a> Serialize for DefUseSnapshot<'a> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        // HashMap<IdentRef, DefId>
        let mut references: HashMap<DefId, Vec<IdentRef>> = {
            let mut map = HashMap::new();
            for (k, v) in &self.0.ident_refs {
                map.entry(*v).or_insert_with(Vec::new).push(k.clone());
            }
            map
        };
        // sort
        for (_, v) in references.iter_mut() {
            v.sort();
        }

        #[derive(Serialize)]
        struct DefUseEntry<'a> {
            def: &'a IdentDef,
            refs: &'a Vec<IdentRef>,
        }

        let mut state = serializer.serialize_map(None)?;
        for (k, (ident_ref, ident_def)) in self.0.ident_defs.as_slice().iter().enumerate() {
            let id = DefId(k as u64);

            let empty_ref = Vec::new();
            let entry = DefUseEntry {
                def: ident_def,
                refs: references.get(&id).unwrap_or(&empty_ref),
            };

            state.serialize_entry(
                &format!(
                    "{}@{}",
                    ident_ref.1,
                    unix_slash(ident_ref.0.vpath().as_rootless_path())
                ),
                &entry,
            )?;
        }

        if !self.0.undefined_refs.is_empty() {
            let mut undefined_refs = self.0.undefined_refs.clone();
            undefined_refs.sort();
            let entry = DefUseEntry {
                def: &IdentDef {
                    name: "<nil>".to_string(),
                    kind: LexicalKind::Block,
                    range: 0..0,
                },
                refs: &undefined_refs,
            };
            state.serialize_entry("<nil>", &entry)?;
        }

        state.end()
    }
}
