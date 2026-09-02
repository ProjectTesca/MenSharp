//! Merging per-file declarations into one symbol table.
//!
//! This is the sequential barrier between "parse and collect every file in parallel"
//! and "resolve types". It exists because two C# features make a single file an
//! incomplete picture: namespaces are open (every file may reopen `A.B`) and
//! `partial` types spread one type over many files. Files are merged in [`FileId`]
//! order, so the table and its diagnostics come out the same no matter how many
//! threads collected the input.
//!
//! Error recovery follows the parser's rule: report, then keep going with the most
//! useful interpretation. A duplicate (non-`partial`) type is reported and then
//! merged as if it were `partial`, so members of both declarations stay visible to
//! later phases instead of one whole declaration vanishing.

use std::collections::HashMap;

use men_sharp_parser::ast::EntityID;

use crate::{
    collect::{DeclarationNode, FileDeclarations, MemberNode, NamespaceNode, TypeNode},
    error::{SemanticError, SemanticErrorKind},
    symbol::{
        Accessibility, DeclarationSite, FileId, Symbol, SymbolId, SymbolKind, SymbolTable,
        SyntaxRef, new_symbol,
    },
};

/// The declaration-level model of a whole compilation: everything the type
/// resolution phase needs, and nothing it has to re-derive.
#[derive(Debug)]
pub struct Declarations<'ast> {
    pub table: SymbolTable<'ast>,
    /// Declaring syntax node -> its symbol. Multiple entities map to one symbol for
    /// namespaces and `partial` types.
    pub symbol_of: HashMap<EntityID, SymbolId>,
    /// The per-file trees, kept because name resolution walks scopes file by file
    /// (`using` directives are file- and namespace-scoped).
    pub files: Vec<FileDeclarations<'ast>>,
    /// Sorted by (file, position); deterministic across thread counts.
    pub errors: Vec<SemanticError>,
    /// The files' names and text, indexed by [`FileId`] — what turns a span
    /// into `File.cs:line:column` for stack traces. Filled by the driver.
    pub sources: Vec<SourceText>,
}

/// One source file as the driver saw it.
#[derive(Debug, Clone)]
pub struct SourceText {
    pub name: std::sync::Arc<str>,
    pub text: std::sync::Arc<str>,
}

impl<'ast> Declarations<'ast> {
    pub fn symbol_of(&self, entity: EntityID) -> Option<SymbolId> {
        self.symbol_of.get(&entity).copied()
    }

    /// Every `global using` in the compilation, in file order. They apply to all
    /// files regardless of where they were written.
    pub fn global_usings(
        &self,
    ) -> impl Iterator<Item = (FileId, &men_sharp_parser::ast::UsingDirective<'ast, 'ast>)> {
        self.files.iter().flat_map(|file| {
            file.usings
                .iter()
                .filter(|using| using.global.is_some())
                .map(move |using| (file.file, *using))
        })
    }
}

pub fn merge_declarations<'ast>(files: Vec<FileDeclarations<'ast>>) -> Declarations<'ast> {
    let mut merger = Merger {
        table: SymbolTable::new(),
        symbol_of: HashMap::new(),
        errors: Vec::new(),
    };

    for file in &files {
        let root = merger.table.root();
        merger.merge_nodes(root, file.file, &file.members);
    }

    merger.check_member_conflicts();

    let mut errors = merger.errors;
    errors.sort_by_key(|error| (error.file, error.span.start, error.span.end));

    Declarations {
        table: merger.table,
        symbol_of: merger.symbol_of,
        files,
        errors,
        sources: Vec::new(),
    }
}

struct Merger<'ast> {
    table: SymbolTable<'ast>,
    symbol_of: HashMap<EntityID, SymbolId>,
    errors: Vec<SemanticError>,
}

impl<'ast> Merger<'ast> {
    fn merge_nodes(&mut self, parent: SymbolId, file: FileId, nodes: &[DeclarationNode<'ast>]) {
        for node in nodes {
            match node {
                DeclarationNode::Namespace(namespace) => {
                    self.merge_namespace(parent, file, namespace)
                }
                DeclarationNode::Type(type_node) => self.merge_type(parent, file, type_node),
            }
        }
    }

    fn merge_namespace(&mut self, parent: SymbolId, file: FileId, node: &NamespaceNode<'ast>) {
        let mut current = parent;

        for (index, segment) in node.name.iter().enumerate() {
            let site = DeclarationSite {
                file,
                span_start: segment.span.start,
                span_end: segment.span.end,
                syntax: SyntaxRef::Namespace(node.syntax),
            };
            current = self.namespace_segment(current, segment.value, site);

            // the declaration as a whole names the innermost namespace
            if index + 1 == node.name.len() {
                self.symbol_of
                    .insert(SyntaxRef::Namespace(node.syntax).entity_id(), current);
            }
        }

        self.merge_nodes(current, file, &node.members);
    }

    /// One dotted segment: reuse the namespace if any file already opened it.
    fn namespace_segment(
        &mut self,
        parent: SymbolId,
        name: &'ast str,
        site: DeclarationSite<'ast>,
    ) -> SymbolId {
        let mut found = None;
        let mut type_conflict = None;
        for &existing in self.table.symbol(parent).members_named(name) {
            let symbol = self.table.symbol(existing);
            match symbol.kind {
                SymbolKind::Namespace => found = Some(existing),
                _ => type_conflict = symbol.declarations.first().copied(),
            }
        }

        if let Some(existing) = found {
            self.table.symbol_mut(existing).declarations.push(site);
            return existing;
        }

        if let Some(first) = type_conflict {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::TypeNamespaceConflict {
                    first_file: first.file,
                    first_span: first.span(),
                },
                file: site.file,
                span: site.span(),
            });
        }

        self.table
            .add_member(parent, new_symbol(SymbolKind::Namespace, name, 0, site))
    }

    fn merge_type(&mut self, parent: SymbolId, file: FileId, node: &TypeNode<'ast>) {
        let site = DeclarationSite {
            file,
            span_start: node.span.start,
            span_end: node.span.end,
            syntax: node.syntax,
        };

        // an earlier declaration of the same (name, arity) merges with this one;
        // whether that was *legal* is a separate question answered below
        let mut existing = None;
        let mut namespace_conflict = None;
        for &candidate in self.table.symbol(parent).members_named(node.name) {
            let symbol = self.table.symbol(candidate);
            if symbol.kind == SymbolKind::Namespace {
                namespace_conflict = symbol.declarations.first().copied();
            } else if symbol.kind.is_type() && symbol.arity == node.arity {
                existing = Some(candidate);
            }
        }

        let id = match existing {
            Some(id) => {
                let symbol = self.table.symbol(id);
                let first = symbol.declarations[0];

                if !(symbol.is_partial && node.is_partial) {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::DuplicateTypeDefinition {
                            first_file: first.file,
                            first_span: first.span(),
                        },
                        file,
                        span: node.span.clone(),
                    });
                } else if symbol.kind != node.kind {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::PartialKindMismatch {
                            first_file: first.file,
                            first_span: first.span(),
                        },
                        file,
                        span: node.span.clone(),
                    });
                }

                let symbol = self.table.symbol_mut(id);
                symbol.declarations.push(site);
                symbol.is_partial &= node.is_partial;
                id
            }
            None => {
                if let Some(first) = namespace_conflict {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::TypeNamespaceConflict {
                            first_file: first.file,
                            first_span: first.span(),
                        },
                        file,
                        span: node.span.clone(),
                    });
                }

                let mut symbol = new_symbol(node.kind, node.name, node.arity, site);
                symbol.is_partial = node.is_partial;
                symbol.is_static = node.is_static;
                symbol.accessibility = node.accessibility.unwrap_or(
                    // top-level types default to internal, nested ones to private
                    // (public inside an interface)
                    match self.table.symbol(parent).kind {
                        SymbolKind::Namespace => Accessibility::Internal,
                        SymbolKind::Interface => Accessibility::Public,
                        _ => Accessibility::Private,
                    },
                );
                let id = self.table.add_member(parent, symbol);

                // type parameters come from the first declaration only; C# requires
                // partial declarations to repeat them identically, which the type
                // resolution phase will verify against each site's syntax
                self.add_type_parameters(id, file, &node.type_parameters);
                id
            }
        };

        self.symbol_of.insert(node.syntax.entity_id(), id);

        for member in &node.members {
            self.merge_member(id, file, member);
        }
        for nested in &node.nested {
            self.merge_type(id, file, nested);
        }
    }

    fn merge_member(&mut self, parent: SymbolId, file: FileId, node: &MemberNode<'ast>) {
        let site = DeclarationSite {
            file,
            span_start: node.span.start,
            span_end: node.span.end,
            syntax: node.syntax,
        };

        let mut symbol = new_symbol(node.kind, node.name, node.arity, site);
        symbol.is_partial = node.is_partial;
        symbol.is_explicit_implementation = node.is_explicit_implementation;
        symbol.is_extension = node.is_extension;
        symbol.is_static = node.is_static;
        symbol.accessibility = node.accessibility.unwrap_or(
            // interface and enum members default to public, everything else private
            match self.table.symbol(parent).kind {
                SymbolKind::Interface | SymbolKind::Enum => Accessibility::Public,
                _ => Accessibility::Private,
            },
        );
        let id = self.table.add_member(parent, symbol);

        self.add_type_parameters(id, file, &node.type_parameters);
        self.symbol_of.insert(node.syntax.entity_id(), id);
    }

    fn add_type_parameters(
        &mut self,
        owner: SymbolId,
        file: FileId,
        parameters: &[&'ast men_sharp_parser::ast::GenericsParameter<'ast, 'ast>],
    ) {
        let mut seen: HashMap<&str, DeclarationSite> = HashMap::new();

        for parameter in parameters {
            let site = DeclarationSite {
                file,
                span_start: parameter.name.span.start,
                span_end: parameter.name.span.end,
                syntax: SyntaxRef::TypeParameter(parameter),
            };

            if let Some(first) = seen.get(parameter.name.value) {
                self.errors.push(SemanticError {
                    kind: SemanticErrorKind::DuplicateTypeParameter {
                        first_file: first.file,
                        first_span: first.span(),
                    },
                    file,
                    span: site.span(),
                });
                continue;
            }
            seen.insert(parameter.name.value, site);

            let mut symbol = new_symbol(SymbolKind::TypeParameter, parameter.name.value, 0, site);
            symbol.variance = match parameter.variance.as_ref().map(|variance| variance.value) {
                Some(men_sharp_parser::ast::Variance::Out) => crate::types::TypeVariance::Covariant,
                Some(men_sharp_parser::ast::Variance::In) => {
                    crate::types::TypeVariance::Contravariant
                }
                None => crate::types::TypeVariance::Invariant,
            };
            let id = self.table.add_type_parameter(owner, symbol);
            self.symbol_of
                .insert(SyntaxRef::TypeParameter(parameter).entity_id(), id);
        }
    }

    /// After every file is merged: names shared inside one container must be either
    /// a family of types differing in arity, or overloads of one member kind.
    ///
    /// Runs at the end rather than during the merge so that members a `partial` type
    /// gained from different files are checked against each other too.
    fn check_member_conflicts(&mut self) {
        let mut errors = Vec::new();

        for (_, symbol) in self.table.iter() {
            if !(symbol.kind.is_type() || symbol.kind == SymbolKind::Namespace) {
                continue;
            }

            for (_, group) in symbol.member_names() {
                let mut first: Option<&Symbol> = None;

                for &id in group {
                    let member = self.table.symbol(id);
                    // explicit implementations live outside ordinary lookup, and a
                    // namespace clash was already reported when it was created
                    if member.is_explicit_implementation || member.kind == SymbolKind::Namespace {
                        continue;
                    }

                    let Some(leader) = first else {
                        first = Some(member);
                        continue;
                    };

                    // types with different arities coexist; duplicates within one
                    // arity were already merged (and reported) above
                    let compatible = if leader.kind.is_type() && member.kind.is_type() {
                        leader.arity != member.arity
                    } else {
                        leader.kind == member.kind && member.kind.is_overloadable()
                    };

                    if !compatible {
                        let first_site = leader.declarations[0];
                        let site = member.declarations[0];
                        errors.push(SemanticError {
                            kind: SemanticErrorKind::DuplicateMemberName {
                                first_file: first_site.file,
                                first_span: first_site.span(),
                            },
                            file: site.file,
                            span: site.span(),
                        });
                    }
                }
            }
        }

        self.errors.append(&mut errors);
    }
}
