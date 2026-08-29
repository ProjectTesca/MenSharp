//! Generic type inference, C# spec §12.6.3.
//!
//! The engine accumulates *bounds* on each method type parameter and then *fixes*
//! them. Three bound flavors, driven by variance:
//!
//! - **lower**: the argument may be *less derived*-convertible into the parameter
//!   (`List<string>` argument against `IEnumerable<T>` fixes `T = string`);
//! - **upper**: the dual, from contravariant positions (`Action<in T>`);
//! - **exact**: invariant positions demand identity.
//!
//! Fixing (§12.6.3.12): exact bounds must agree; otherwise the candidate set is
//! the lower and upper bounds filtered by convertibility, and the winner is the
//! unique candidate every other candidate converts *to*.
//!
//! The same machinery gives *best common type* (§12.6.3.15) — conditional arms,
//! implicitly typed arrays, lambda `return` statements — by treating the result as
//! one fresh type parameter with each candidate as a lower bound.
//!
//! Lambdas do not participate here directly: the body checker runs the spec's
//! two-phase dance (fix what the ordinary arguments determine, type lambda bodies
//! against the now-concrete parameter types, feed their return types back as lower
//! bounds, fix the rest). This module only supplies the bound arithmetic.

use std::collections::HashMap;

use crate::{
    lookup::TypeSystem,
    symbol::SymbolId,
    types::{MemberSignature, Type, TypeTarget, TypeVariance},
};

/// Identifies one method type parameter being inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InferenceKey {
    /// A source method's type parameter symbol.
    Source(SymbolId),
    /// An external method's `!!n`.
    External(u32),
}

#[derive(Debug, Default, Clone)]
struct Bounds {
    exact: Vec<Type>,
    lower: Vec<Type>,
    upper: Vec<Type>,
}

pub(crate) struct Inference {
    keys: Vec<InferenceKey>,
    bounds: HashMap<InferenceKey, Bounds>,
    fixed: HashMap<InferenceKey, Type>,
}

impl Inference {
    pub fn new(keys: Vec<InferenceKey>) -> Self {
        Self {
            keys,
            bounds: HashMap::new(),
            fixed: HashMap::new(),
        }
    }

    fn key_of(&self, ty: &Type) -> Option<InferenceKey> {
        let key = match ty {
            Type::TypeParameter(symbol) => InferenceKey::Source(*symbol),
            Type::ExternalMethodTypeParameter(index) => InferenceKey::External(*index),
            _ => return None,
        };
        self.keys.contains(&key).then_some(key)
    }

    /// Whether `ty` still mentions an unfixed parameter — a lambda argument whose
    /// delegate type looks like this is not ready for its body to be typed.
    pub fn has_unfixed(&self, ty: &Type) -> bool {
        let found = std::cell::Cell::new(false);
        ty.map(&|node| {
            if let Some(key) = self.key_of(&node)
                && !self.fixed.contains_key(&key)
            {
                found.set(true);
            }
            node
        });
        found.get()
    }

    /// The current substitution applied to a type: fixed parameters replaced,
    /// unfixed ones left alone.
    pub fn substitute(&self, ty: &Type) -> Type {
        ty.map(&|node| match self.key_of(&node) {
            Some(key) => self.fixed.get(&key).cloned().unwrap_or(node),
            None => node,
        })
    }

    pub fn substitute_signature(&self, signature: &MemberSignature) -> MemberSignature {
        signature.map(&|node| match self.key_of(&node) {
            Some(key) => self.fixed.get(&key).cloned().unwrap_or(node),
            None => node,
        })
    }

    // -------------------------------------------------------------- bounds

    /// §12.6.3.10 lower-bound inference: `argument` flows into `parameter`.
    pub fn lower_bound(&mut self, system: &TypeSystem, parameter: &Type, argument: &Type) {
        // no information flows out of these
        if matches!(argument, Type::Null | Type::Error | Type::Infer) {
            return;
        }

        if let Some(key) = self.key_of(parameter) {
            self.bounds
                .entry(key)
                .or_default()
                .lower
                .push(argument.clone());
            return;
        }

        match (parameter, argument) {
            (
                Type::Array {
                    element: parameter_element,
                    rank,
                },
                Type::Array {
                    element: argument_element,
                    rank: argument_rank,
                },
            ) if rank == argument_rank => {
                if system.is_reference_type(argument_element) {
                    self.lower_bound(system, parameter_element, argument_element);
                } else {
                    self.exact(system, parameter_element, argument_element);
                }
            }
            (Type::Nullable(parameter_inner), Type::Nullable(argument_inner)) => {
                self.exact(system, parameter_inner, argument_inner);
            }
            (Type::Tuple(parameter_elements), Type::Tuple(argument_elements))
                if parameter_elements.len() == argument_elements.len() =>
            {
                for (parameter, argument) in parameter_elements.iter().zip(argument_elements) {
                    self.exact(system, &parameter.element, &argument.element);
                }
            }
            (
                Type::Named {
                    target: parameter_target,
                    arguments: parameter_arguments,
                },
                _,
            ) if !parameter_arguments.is_empty() => {
                // a single-dimensional array argument satisfies IEnumerable<T> and
                // friends through the array interfaces
                if let Type::Array { element, rank: 1 } = argument
                    && self.is_array_interface(system, parameter_target)
                    && parameter_arguments.len() == 1
                {
                    if system.is_reference_type(element) {
                        self.lower_bound(system, &parameter_arguments[0], element);
                    } else {
                        self.exact(system, &parameter_arguments[0], element);
                    }
                    return;
                }

                // find the unique instantiation of the parameter's generic type in
                // the argument's inheritance closure
                let Some(found_arguments) =
                    unique_instantiation(system, argument, parameter_target)
                else {
                    return;
                };
                if found_arguments.len() != parameter_arguments.len() {
                    return;
                }

                let variances = variances_of(system, parameter_target, parameter_arguments.len());
                for ((parameter, argument), variance) in parameter_arguments
                    .iter()
                    .zip(&found_arguments)
                    .zip(variances)
                {
                    match variance {
                        TypeVariance::Covariant if system.is_reference_type(argument) => {
                            self.lower_bound(system, parameter, argument)
                        }
                        TypeVariance::Contravariant if system.is_reference_type(argument) => {
                            self.upper_bound(system, parameter, argument)
                        }
                        _ => self.exact(system, parameter, argument),
                    }
                }
            }
            _ => {}
        }
    }

    /// §12.6.3.11 upper-bound inference (the contravariant dual).
    pub fn upper_bound(&mut self, system: &TypeSystem, parameter: &Type, argument: &Type) {
        if matches!(argument, Type::Null | Type::Error | Type::Infer) {
            return;
        }

        if let Some(key) = self.key_of(parameter) {
            self.bounds
                .entry(key)
                .or_default()
                .upper
                .push(argument.clone());
            return;
        }

        match (parameter, argument) {
            (
                Type::Named {
                    target: parameter_target,
                    arguments: parameter_arguments,
                },
                Type::Named {
                    target: argument_target,
                    arguments: argument_arguments,
                },
            ) if parameter_target == argument_target
                && parameter_arguments.len() == argument_arguments.len() =>
            {
                let variances = variances_of(system, parameter_target, parameter_arguments.len());
                for ((parameter, argument), variance) in parameter_arguments
                    .iter()
                    .zip(argument_arguments)
                    .zip(variances)
                {
                    match variance {
                        TypeVariance::Covariant => self.upper_bound(system, parameter, argument),
                        TypeVariance::Contravariant => {
                            self.lower_bound(system, parameter, argument)
                        }
                        TypeVariance::Invariant => self.exact(system, parameter, argument),
                    }
                }
            }
            _ => self.exact(system, parameter, argument),
        }
    }

    /// §12.6.3.9 exact inference.
    #[allow(
        clippy::only_used_in_recursion,
        reason = "the parameter keeps all three inference forms uniform"
    )]
    pub fn exact(&mut self, system: &TypeSystem, parameter: &Type, argument: &Type) {
        if matches!(argument, Type::Null | Type::Error | Type::Infer) {
            return;
        }

        if let Some(key) = self.key_of(parameter) {
            self.bounds
                .entry(key)
                .or_default()
                .exact
                .push(argument.clone());
            return;
        }

        match (parameter, argument) {
            (
                Type::Array {
                    element: parameter_element,
                    rank,
                },
                Type::Array {
                    element: argument_element,
                    rank: argument_rank,
                },
            ) if rank == argument_rank => self.exact(system, parameter_element, argument_element),
            (Type::Nullable(parameter_inner), Type::Nullable(argument_inner)) => {
                self.exact(system, parameter_inner, argument_inner)
            }
            (Type::Pointer(parameter_inner), Type::Pointer(argument_inner)) => {
                self.exact(system, parameter_inner, argument_inner)
            }
            (
                Type::ByRef {
                    element: parameter_element,
                    ..
                },
                _,
            ) => self.exact(system, parameter_element, argument),
            (Type::Tuple(parameter_elements), Type::Tuple(argument_elements))
                if parameter_elements.len() == argument_elements.len() =>
            {
                for (parameter, argument) in parameter_elements.iter().zip(argument_elements) {
                    self.exact(system, &parameter.element, &argument.element);
                }
            }
            (
                Type::Named {
                    target: parameter_target,
                    arguments: parameter_arguments,
                },
                Type::Named {
                    target: argument_target,
                    arguments: argument_arguments,
                },
            ) if parameter_target == argument_target
                && parameter_arguments.len() == argument_arguments.len() =>
            {
                for (parameter, argument) in parameter_arguments.iter().zip(argument_arguments) {
                    self.exact(system, parameter, argument);
                }
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------- fixing

    /// §12.6.3.12: tries to fix every key that has bounds but no fixed type yet.
    pub fn fix_where_possible(&mut self, system: &TypeSystem) {
        for key in self.keys.clone() {
            if self.fixed.contains_key(&key) {
                continue;
            }
            if let Some(fixed) = self.try_fix(system, key) {
                self.fixed.insert(key, fixed);
            }
        }
    }

    pub fn all_fixed(&self) -> bool {
        self.keys.iter().all(|key| self.fixed.contains_key(key))
    }

    /// Fixes a parameter directly — explicitly written type arguments.
    pub fn preset(&mut self, key: InferenceKey, ty: Type) {
        self.fixed.insert(key, ty);
    }

    fn try_fix(&self, system: &TypeSystem, key: InferenceKey) -> Option<Type> {
        let bounds = self.bounds.get(&key)?;

        // bounds may mention parameters fixed in an earlier round
        let exact: Vec<Type> = bounds.exact.iter().map(|ty| self.substitute(ty)).collect();
        let lower: Vec<Type> = bounds.lower.iter().map(|ty| self.substitute(ty)).collect();
        let upper: Vec<Type> = bounds.upper.iter().map(|ty| self.substitute(ty)).collect();

        if let Some(first) = exact.first() {
            return exact.iter().all(|ty| ty == first).then(|| first.clone());
        }

        let mut candidates: Vec<Type> = Vec::new();
        for candidate in lower.iter().chain(&upper) {
            if !candidates.contains(candidate) {
                candidates.push(candidate.clone());
            }
        }

        candidates.retain(|candidate| {
            lower
                .iter()
                .all(|bound| system.is_implicitly_convertible(bound, candidate))
                && upper
                    .iter()
                    .all(|bound| system.is_implicitly_convertible(candidate, bound))
        });

        // the unique candidate every other candidate converts to
        let mut winner: Option<&Type> = None;
        for candidate in &candidates {
            if candidates
                .iter()
                .all(|other| system.is_implicitly_convertible(other, candidate))
            {
                match winner {
                    None => winner = Some(candidate),
                    Some(existing) if existing == candidate => {}
                    Some(_) => return None, // two incomparable winners
                }
            }
        }
        winner.cloned()
    }
}

/// §12.6.3.15 best common type: the arms of a conditional, the elements of an
/// implicitly typed array, the returns of an inferred lambda.
pub(crate) fn best_common_type(system: &TypeSystem, types: &[Type]) -> Option<Type> {
    let informative: Vec<&Type> = types
        .iter()
        .filter(|ty| !matches!(ty, Type::Null | Type::Infer))
        .collect();
    if informative.is_empty() {
        return None;
    }
    if informative.iter().any(|ty| matches!(ty, Type::Error)) {
        return Some(Type::Error);
    }

    // one fresh parameter, every type a lower bound
    let key = InferenceKey::External(u32::MAX);
    let mut inference = Inference::new(vec![key]);
    let fresh = Type::ExternalMethodTypeParameter(u32::MAX);
    for ty in &informative {
        inference.lower_bound(system, &fresh, ty);
    }
    inference.fix_where_possible(system);
    inference.fixed.get(&key).cloned()
}

/// The unique `Named` instantiation of `wanted` in `ty`'s inheritance closure
/// (itself, base classes, interfaces).
fn unique_instantiation(system: &TypeSystem, ty: &Type, wanted: &TypeTarget) -> Option<Vec<Type>> {
    let mut found: Option<Vec<Type>> = None;
    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![ty.clone()];

    while let Some(current) = queue.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        if let Type::Named { target, arguments } = &current
            && target == wanted
        {
            match &found {
                None => found = Some(arguments.clone()),
                Some(existing) if existing == arguments => {}
                Some(_) => return None, // ambiguous: two different instantiations
            }
        }
        queue.extend(system.interfaces_of(&current));
        if let Some(base) = system.base_of(&current) {
            queue.push(base);
        }
    }

    found
}

fn variances_of(system: &TypeSystem, target: &TypeTarget, count: usize) -> Vec<TypeVariance> {
    let mut variances = match target {
        TypeTarget::Source(symbol) => system
            .declarations
            .table
            .symbol(*symbol)
            .type_parameters
            .iter()
            .map(|&parameter| system.declarations.table.symbol(parameter).variance)
            .collect(),
        TypeTarget::External(id) => system.external.variances(*id),
    };
    variances.resize(count, TypeVariance::Invariant);
    variances
}

impl Inference {
    /// Whether the parameter's target is one of the interfaces single-dimensional
    /// arrays implement (`IEnumerable<T>`, `ICollection<T>`, `IList<T>`, and their
    /// read-only versions).
    fn is_array_interface(&self, system: &TypeSystem, target: &TypeTarget) -> bool {
        let TypeTarget::External(id) = target else {
            return false;
        };
        [
            "IEnumerable",
            "ICollection",
            "IList",
            "IReadOnlyCollection",
            "IReadOnlyList",
        ]
        .iter()
        .any(|name| {
            system
                .external
                .find_type(&["System", "Collections", "Generic"], name, 1)
                == Some(*id)
        })
    }
}
