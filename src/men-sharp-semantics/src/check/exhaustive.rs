//! Does a `switch` handle every case?
//!
//! C# asks that of a switch *expression* only, and only as a warning, for
//! the shapes it can enumerate: `bool`, an enum's named members. Here it is
//! an error, and it extends to a `[Union]` type — an abstract class or an
//! interface whose set of concrete types is closed, so that `shape switch
//! { Circle c => ..., Square s => ... }` is complete when `Circle` and
//! `Square` are all there is. A switch *statement* over a `[Union]` is held
//! to the same standard: the attribute is a request to be told when a case
//! is forgotten, and a statement is where most of them are handled.
//!
//! The analysis is deliberately simple. A case is covered when an arm
//! without a `when` guard covers it: a discard, `var`, a type the case
//! converts to, the enum member itself, `not`/`and`/`or` of those, or a
//! property/positional pattern whose parts all always match. Relational
//! patterns and list patterns cover nothing — the message says which cases
//! are missing, and `_ =>` is always accepted.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use men_sharp_parser::ast::{
    Attribute, AttributeSection, EntityID, Expression, LiteralExpression, Modifier, Pattern,
    PrimaryLeft, SwitchExpressionArm, SwitchLabel, SwitchSection, TypeRef, TypeRefBase,
};

use super::{Checker, ResolvedTarget};
use crate::error::SemanticErrorKind;
use crate::lookup::MemberOrigin;
use crate::symbol::{SymbolId, SymbolKind, SyntaxRef};
use crate::types::{Type, TypeTarget};

/// Everything a switch over one type has to handle.
struct Domain {
    /// The switched type, for `object o`-style patterns that take all of it.
    subject: Type,
    cases: Vec<Case>,
}

struct Case {
    /// How the case is named in the error.
    name: String,
    shape: CaseShape,
}

enum CaseShape {
    Bool(bool),
    /// One enum value; every member that has it, since aliases match alike.
    EnumValue(Vec<SymbolId>),
    /// A concrete type of a `[Union]`.
    Type(Type),
}

/// The last segment of an attribute's name, so `[MenSharp.Union]` and
/// `[Union]` read the same.
fn attribute_name<'a>(attribute: &'a Attribute<'a, 'a>) -> Option<&'a str> {
    let TypeRefBase::Name(name) = &attribute.name.base else {
        return None;
    };
    let spelling = name.segments.last()?.name.value;
    Some(spelling.strip_suffix("Attribute").unwrap_or(spelling))
}

/// The `[Union]` attribute among these sections, if any.
fn union_attribute<'a>(sections: &'a [AttributeSection<'a, 'a>]) -> Option<&'a Attribute<'a, 'a>> {
    sections
        .iter()
        .flat_map(|section| section.attributes)
        .find(|attribute| attribute_name(attribute) == Some("Union"))
}

/// `true`/`false` written as a pattern.
fn bool_literal(expression: &Expression) -> Option<bool> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if !primary.chain.is_empty() {
        return None;
    }
    match primary.left {
        PrimaryLeft::Literal(LiteralExpression::True(_)) => Some(true),
        PrimaryLeft::Literal(LiteralExpression::False(_)) => Some(false),
        _ => None,
    }
}

/// The value of an integer literal, as an enum member initializer is
/// allowed to be. Anything else is `None`: the member is then a case of
/// its own, which is right unless it aliases another.
fn integer_literal_value(expression: &Expression) -> Option<i64> {
    let Expression::Primary(primary) = expression else {
        return None;
    };
    if !primary.chain.is_empty() {
        return None;
    }
    let PrimaryLeft::Literal(LiteralExpression::Integer(text)) = &primary.left else {
        return None;
    };
    let stripped = text.value.trim_end_matches(['u', 'U', 'l', 'L']);
    let digits: String = stripped.chars().filter(|c| *c != '_').collect();
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).ok()
    } else if let Some(bits) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        i64::from_str_radix(bits, 2).ok()
    } else {
        digits.parse().ok()
    }
}

/// Binds a type's parameters so that `written` (a base written in terms
/// of them) becomes `target`. `None` when the shapes do not fit.
fn unify(
    written: &Type,
    target: &Type,
    parameters: &[SymbolId],
    bindings: &mut HashMap<SymbolId, Type>,
) -> bool {
    match (written, target) {
        (Type::TypeParameter(parameter), _) if parameters.contains(parameter) => {
            match bindings.get(parameter) {
                Some(bound) => bound == target,
                None => {
                    bindings.insert(*parameter, target.clone());
                    true
                }
            }
        }
        (
            Type::Named {
                target: a,
                arguments: x,
            },
            Type::Named {
                target: b,
                arguments: y,
            },
        ) => {
            a == b
                && x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(x, y)| unify(x, y, parameters, bindings))
        }
        (
            Type::Array {
                element: a,
                rank: x,
            },
            Type::Array {
                element: b,
                rank: y,
            },
        ) => x == y && unify(a, b, parameters, bindings),
        (Type::Nullable(a), Type::Nullable(b)) => unify(a, b, parameters, bindings),
        (Type::Tuple(a), Type::Tuple(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| unify(&a.element, &b.element, parameters, bindings))
        }
        _ => written == target,
    }
}

impl<'a, 'ast> Checker<'a, 'ast> {
    // ------------------------------------------------------------ [Union]

    /// Is this source type marked `[Union]`?
    pub(super) fn is_union_type(&self, symbol: SymbolId) -> bool {
        self.resolver
            .declarations
            .table
            .symbol(symbol)
            .declarations
            .iter()
            .any(|site| match &site.syntax {
                SyntaxRef::Class(declaration) => union_attribute(declaration.attributes).is_some(),
                _ => false,
            })
    }

    /// `[Union]` goes on an abstract class or an interface: a type nothing
    /// is an instance of, so that its cases are exactly the types below it.
    pub(super) fn check_union_attribute(&mut self, symbol: SymbolId) {
        let entry = self.resolver.declarations.table.symbol(symbol);
        let mut misplaced: Vec<Range<usize>> = Vec::new();
        for site in &entry.declarations {
            let SyntaxRef::Class(declaration) = &site.syntax else {
                continue;
            };
            let Some(attribute) = union_attribute(declaration.attributes) else {
                continue;
            };
            let allowed = entry.kind == SymbolKind::Interface
                || (matches!(entry.kind, SymbolKind::Class | SymbolKind::Record)
                    && self.is_abstract_type(symbol));
            if !allowed {
                misplaced.push(attribute.span.clone());
            }
        }
        for span in misplaced {
            self.error(SemanticErrorKind::UnionNotAbstract, span);
        }
    }

    fn is_sealed_type(&self, symbol: SymbolId) -> bool {
        self.resolver
            .declarations
            .table
            .symbol(symbol)
            .declarations
            .iter()
            .any(|site| {
                matches!(&site.syntax, SyntaxRef::Class(declaration)
                if declaration.modifiers.iter().any(|modifier| modifier.value == Modifier::Sealed))
            })
    }

    /// The concrete types below `union`, instantiated the way `union` is:
    /// its cases. A class that is neither abstract nor sealed is a case and
    /// may have more below it, which are cases of their own — a `Foo f`
    /// pattern takes them all, a `Bar b` only `Bar`.
    fn union_cases(
        &mut self,
        union: &Type,
        out: &mut Vec<Type>,
        visited: &mut HashSet<Type>,
        span: &Range<usize>,
    ) {
        let table = &self.resolver.declarations.table;
        let candidates: Vec<(SymbolId, Vec<Type>)> = table
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.kind,
                    SymbolKind::Class
                        | SymbolKind::Record
                        | SymbolKind::Struct
                        | SymbolKind::RecordStruct
                        | SymbolKind::Interface
                )
            })
            .filter_map(|(id, _)| Some((id, self.signatures.base_types.get(&id)?.clone())))
            .collect();

        let Type::Named {
            target: want,
            arguments: want_arguments,
        } = union
        else {
            return;
        };

        for (candidate, bases) in candidates {
            let parameters = self.system().source_type_parameters(candidate);
            for base in &bases {
                let Type::Named { target, arguments } = base else {
                    continue;
                };
                if target != want || arguments.len() != want_arguments.len() {
                    continue;
                }
                let mut bindings: HashMap<SymbolId, Type> = HashMap::new();
                let fits = arguments
                    .iter()
                    .zip(want_arguments)
                    .all(|(written, wanted)| unify(written, wanted, &parameters, &mut bindings));
                if !fits {
                    continue;
                }
                let arguments: Option<Vec<Type>> = parameters
                    .iter()
                    .map(|parameter| bindings.get(parameter).cloned())
                    .collect();
                let Some(arguments) = arguments else {
                    // `class Weird<U> : Option<int>`: which `Weird<U>` are
                    // cases of `Option<int>`? Every one — not a list
                    let kind = SemanticErrorKind::UnionCaseUndetermined {
                        case: self
                            .resolver
                            .declarations
                            .table
                            .fully_qualified_name(candidate),
                    };
                    self.error(kind, span.clone());
                    continue;
                };
                let instantiated = Type::Named {
                    target: TypeTarget::Source(candidate),
                    arguments,
                };
                if !visited.insert(instantiated.clone()) {
                    continue;
                }
                let entry = self.resolver.declarations.table.symbol(candidate);
                let concrete =
                    !(entry.kind == SymbolKind::Interface || self.is_abstract_type(candidate));
                if concrete {
                    out.push(instantiated.clone());
                }
                let leaf = matches!(entry.kind, SymbolKind::Struct | SymbolKind::RecordStruct)
                    || self.is_sealed_type(candidate);
                if !leaf {
                    self.union_cases(&instantiated, out, visited, span);
                }
            }
        }
    }

    /// Does a value of type `sub` convert to `sup` by identity or by
    /// reference — is a `sup`-typed pattern sure to take it?
    fn is_subtype(&self, sub: &Type, sup: &Type) -> bool {
        let system = self.system();
        let mut stack = vec![sub.clone()];
        let mut seen: HashSet<Type> = HashSet::new();
        while let Some(current) = stack.pop() {
            if &current == sup {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            stack.extend(system.base_of(&current));
            stack.extend(system.interfaces_of(&current));
        }
        false
    }

    // ------------------------------------------------------------ domains

    /// What a switch over `value` has to cover, or `None` when there is no
    /// finite list to hold it to. A statement is only held to a `[Union]`.
    fn switch_domain(
        &mut self,
        value: &Type,
        statement: bool,
        span: &Range<usize>,
    ) -> Option<Domain> {
        if !statement && self.system().is_bool(value) {
            return Some(Domain {
                subject: value.clone(),
                cases: [true, false]
                    .into_iter()
                    .map(|truth| Case {
                        name: truth.to_string(),
                        shape: CaseShape::Bool(truth),
                    })
                    .collect(),
            });
        }
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = value
        else {
            return None;
        };
        let symbol = *symbol;
        match self.resolver.declarations.table.symbol(symbol).kind {
            SymbolKind::Enum if !statement => Some(Domain {
                subject: value.clone(),
                cases: self.enum_cases(symbol, value),
            }),
            SymbolKind::Class | SymbolKind::Record | SymbolKind::Interface
                if self.is_union_type(symbol) =>
            {
                let mut types = Vec::new();
                self.union_cases(value, &mut types, &mut HashSet::new(), span);
                let cases = types
                    .into_iter()
                    .map(|ty| Case {
                        name: self.display(&ty),
                        shape: CaseShape::Type(ty),
                    })
                    .collect();
                Some(Domain {
                    subject: value.clone(),
                    cases,
                })
            }
            _ => None,
        }
    }

    /// An enum's named values, in declaration order; members sharing a
    /// value are one case, named after the first of them.
    fn enum_cases(&self, symbol: SymbolId, enum_type: &Type) -> Vec<Case> {
        let table = &self.resolver.declarations.table;
        let type_name = self.display(enum_type);
        let mut cases: Vec<(Option<i64>, Case)> = Vec::new();
        let mut next: i64 = 0;
        for &member in &table.symbol(symbol).members {
            let entry = table.symbol(member);
            if entry.kind != SymbolKind::EnumMember {
                continue;
            }
            let initializer = entry
                .declarations
                .first()
                .and_then(|site| match &site.syntax {
                    SyntaxRef::EnumMember(node) => node.value.as_ref(),
                    _ => None,
                });
            let value = match initializer {
                None => Some(next),
                Some(expression) => integer_literal_value(expression),
            };
            if let Some(value) = value {
                next = value.wrapping_add(1);
            }
            let alias = value.and_then(|value| {
                cases
                    .iter_mut()
                    .find(|(existing, _)| *existing == Some(value))
            });
            match alias {
                Some((_, case)) => {
                    if let CaseShape::EnumValue(members) = &mut case.shape {
                        members.push(member);
                    }
                }
                None => cases.push((
                    value,
                    Case {
                        name: format!("{type_name}.{}", entry.name),
                        shape: CaseShape::EnumValue(vec![member]),
                    },
                )),
            }
        }
        cases.into_iter().map(|(_, case)| case).collect()
    }

    // ----------------------------------------------------------- coverage

    /// The type a pattern's type part resolved to when the pattern was
    /// checked; nothing when it did not resolve.
    fn pattern_type_of(&self, pattern_type: &'ast TypeRef<'ast, 'ast>) -> Option<Type> {
        self.resolver
            .out
            .type_of
            .get(&EntityID::from(pattern_type))
            .cloned()
    }

    /// Marks the cases a pattern is sure to take.
    fn cover(&self, pattern: &'ast Pattern<'ast, 'ast>, domain: &Domain, covered: &mut [bool]) {
        match pattern {
            Pattern::Discard(_) | Pattern::Var { .. } => covered.fill(true),
            Pattern::Declaration {
                pattern_type,
                designation,
                ..
            } => {
                // `Color.Red`: a constant, bound as a member on the type node
                if designation.is_none()
                    && let Some(target) = self.targets.get(&EntityID::from(pattern_type))
                {
                    if let ResolvedTarget::Member(member) = target
                        && member.kind == SymbolKind::EnumMember
                        && let MemberOrigin::Source(symbol) = member.origin
                    {
                        for (index, case) in domain.cases.iter().enumerate() {
                            if let CaseShape::EnumValue(members) = &case.shape
                                && members.contains(&symbol)
                            {
                                covered[index] = true;
                            }
                        }
                    }
                    return;
                }
                if let Some(ty) = self.pattern_type_of(pattern_type) {
                    self.cover_type(&ty, domain, covered);
                }
            }
            Pattern::Constant(expression) => {
                if let Some(truth) = bool_literal(expression) {
                    for (index, case) in domain.cases.iter().enumerate() {
                        if matches!(case.shape, CaseShape::Bool(value) if value == truth) {
                            covered[index] = true;
                        }
                    }
                }
            }
            Pattern::Relational { .. } | Pattern::List { .. } | Pattern::Slice { .. } => {}
            Pattern::Not { pattern, .. } => {
                if let Ok(pattern) = pattern {
                    let mut inner = vec![false; covered.len()];
                    self.cover(pattern, domain, &mut inner);
                    for (slot, taken) in covered.iter_mut().zip(inner) {
                        *slot |= !taken;
                    }
                }
            }
            Pattern::And { left, right, .. } => {
                if let Ok(right) = right {
                    let mut first = vec![false; covered.len()];
                    let mut second = vec![false; covered.len()];
                    self.cover(left, domain, &mut first);
                    self.cover(right, domain, &mut second);
                    for ((slot, a), b) in covered.iter_mut().zip(first).zip(second) {
                        *slot |= a && b;
                    }
                }
            }
            Pattern::Or { left, right, .. } => {
                self.cover(left, domain, covered);
                if let Ok(right) = right {
                    self.cover(right, domain, covered);
                }
            }
            Pattern::Parenthesized { pattern, .. } => {
                if let Ok(pattern) = pattern {
                    self.cover(pattern, domain, covered);
                }
            }
            Pattern::Property {
                pattern_type,
                subpatterns,
                ..
            } => {
                let total = subpatterns.iter().all(|subpattern| {
                    subpattern
                        .pattern
                        .as_ref()
                        .is_ok_and(|pattern| self.is_irrefutable(pattern))
                });
                if total {
                    self.cover_type_part(pattern_type.as_ref(), domain, covered);
                }
            }
            Pattern::Positional {
                pattern_type,
                subpatterns,
                property_subpatterns,
                ..
            } => {
                let total = subpatterns
                    .iter()
                    .all(|subpattern| self.is_irrefutable(&subpattern.pattern))
                    && property_subpatterns.iter().all(|subpattern| {
                        subpattern
                            .pattern
                            .as_ref()
                            .is_ok_and(|pattern| self.is_irrefutable(pattern))
                    });
                if total {
                    self.cover_type_part(pattern_type.as_ref(), domain, covered);
                }
            }
        }
    }

    /// `Type { ... }` / `Type(...)` with parts that always match: covers
    /// what a plain `Type` pattern would; with no type, everything.
    fn cover_type_part(
        &self,
        pattern_type: Option<&'ast TypeRef<'ast, 'ast>>,
        domain: &Domain,
        covered: &mut [bool],
    ) {
        match pattern_type {
            None => covered.fill(true),
            Some(pattern_type) => {
                if let Some(ty) = self.pattern_type_of(pattern_type) {
                    self.cover_type(&ty, domain, covered);
                }
            }
        }
    }

    /// The cases a type pattern takes: those that convert to the type.
    fn cover_type(&self, ty: &Type, domain: &Domain, covered: &mut [bool]) {
        if self.is_subtype(&domain.subject, ty) {
            covered.fill(true);
            return;
        }
        for (index, case) in domain.cases.iter().enumerate() {
            if let CaseShape::Type(case_type) = &case.shape
                && self.is_subtype(case_type, ty)
            {
                covered[index] = true;
            }
        }
    }

    /// A pattern that matches whatever it is given — the parts a property
    /// or positional pattern may have and still cover its type.
    fn is_irrefutable(&self, pattern: &'ast Pattern<'ast, 'ast>) -> bool {
        match pattern {
            Pattern::Discard(_) | Pattern::Var { .. } => true,
            Pattern::Parenthesized { pattern, .. } => pattern
                .as_ref()
                .is_ok_and(|pattern| self.is_irrefutable(pattern)),
            // `int x` on an `int` part
            Pattern::Declaration { pattern_type, .. } => {
                let input = self.pattern_inputs.get(&EntityID::from(pattern));
                match (input, self.pattern_type_of(pattern_type)) {
                    (Some(input), Some(ty)) => self.is_subtype(input, &ty),
                    _ => false,
                }
            }
            Pattern::Property {
                pattern_type: None,
                subpatterns,
                ..
            } => subpatterns.iter().all(|subpattern| {
                subpattern
                    .pattern
                    .as_ref()
                    .is_ok_and(|pattern| self.is_irrefutable(pattern))
            }),
            Pattern::Positional {
                pattern_type: None,
                subpatterns,
                property_subpatterns,
                ..
            } => {
                subpatterns
                    .iter()
                    .all(|subpattern| self.is_irrefutable(&subpattern.pattern))
                    && property_subpatterns.iter().all(|subpattern| {
                        subpattern
                            .pattern
                            .as_ref()
                            .is_ok_and(|pattern| self.is_irrefutable(pattern))
                    })
            }
            Pattern::And { left, right, .. } => {
                self.is_irrefutable(left)
                    && right.as_ref().is_ok_and(|right| self.is_irrefutable(right))
            }
            Pattern::Or { left, right, .. } => {
                self.is_irrefutable(left)
                    || right.as_ref().is_ok_and(|right| self.is_irrefutable(right))
            }
            _ => false,
        }
    }

    fn report_missing(&mut self, domain: &Domain, covered: &[bool], span: Range<usize>) {
        let missing: Vec<String> = domain
            .cases
            .iter()
            .zip(covered)
            .filter(|(_, taken)| !**taken)
            .map(|(case, _)| case.name.clone())
            .collect();
        if missing.is_empty() {
            return;
        }
        let kind = SemanticErrorKind::NonExhaustiveSwitch {
            subject: self.display(&domain.subject),
            missing,
        };
        self.error(kind, span);
    }

    // ----------------------------------------------------------- entry points

    /// A switch expression has no fall-through: an input no arm takes is
    /// a runtime halt, so every case must have an arm.
    pub(super) fn check_switch_expression_exhaustive(
        &mut self,
        value: &Type,
        arms: &'ast [SwitchExpressionArm<'ast, 'ast>],
        span: Range<usize>,
    ) {
        let Some(domain) = self.switch_domain(value, false, &span) else {
            return;
        };
        let mut covered = vec![false; domain.cases.len()];
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            self.cover(&arm.pattern, &domain, &mut covered);
        }
        self.report_missing(&domain, &covered, span);
    }

    /// A switch statement over a `[Union]`: every case, or `default`.
    pub(super) fn check_switch_statement_exhaustive(
        &mut self,
        value: &Type,
        sections: &'ast [SwitchSection<'ast, 'ast>],
        span: Range<usize>,
    ) {
        let Some(domain) = self.switch_domain(value, true, &span) else {
            return;
        };
        let mut covered = vec![false; domain.cases.len()];
        for section in sections {
            for label in section.labels {
                match label {
                    SwitchLabel::Default { .. } => covered.fill(true),
                    SwitchLabel::Case { pattern, guard, .. } => {
                        if guard.is_some() {
                            continue;
                        }
                        if let Ok(pattern) = pattern {
                            self.cover(pattern, &domain, &mut covered);
                        }
                    }
                }
            }
        }
        self.report_missing(&domain, &covered, span);
    }
}
