//! Calls: which overload a call means, and what it takes to get there.

use super::{
    AccessContext, Applicable, ArgumentShape, AttemptOutcome, CallArgument, Checker, Meaning,
    MethodGroup, ResolvedCall, ResolvedTarget, SelectedOverload, method_parameter_keys,
    type_parameter_leaves,
};
use crate::error::SemanticErrorKind;
use crate::semantics::resolve::Resolution;
use crate::symbol::{SymbolId, SymbolKind};
use crate::types::infer::{Inference, InferenceKey};
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{FunctionSignature, MemberSignature, ParameterPassing, Type, TypeTarget};
use men_sharp_parser::ast::{Argument, ArgumentModifier, ArgumentValue, EntityID, Expression};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    pub(super) fn invoke(
        &mut self,
        meaning: Meaning<'ast>,
        arguments: &'ast [Argument<'ast, 'ast>],
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Meaning<'ast> {
        match meaning {
            Meaning::Group(group) => {
                let call_arguments = self.check_arguments(arguments);
                Meaning::Value(self.resolve_call(group, call_arguments, span, node))
            }
            Meaning::Value(ty) => {
                // calling a value: a delegate invocation
                let invoke = self
                    .system()
                    .members_named(&ty, "Invoke")
                    .into_iter()
                    .find(|candidate| candidate.kind == SymbolKind::Method)
                    .or_else(|| self.source_delegate_invoke(&ty));
                match invoke {
                    Some(candidate) => {
                        let receiver_display = self.describe(&ty);
                        let group = MethodGroup {
                            candidates: vec![candidate],
                            explicit_arguments: Vec::new(),
                            via_type: false,
                            name: "Invoke",
                            receiver: None,
                            allow_extensions: false,
                            receiver_display,
                            span: span.clone(),
                        };
                        let call_arguments = self.check_arguments(arguments);
                        Meaning::Value(self.resolve_call(group, call_arguments, span, node))
                    }
                    None => {
                        if !matches!(ty, Type::Error) {
                            let kind = SemanticErrorKind::NotCallable {
                                type_name: self.describe(&ty),
                            };
                            self.error(kind, span.clone());
                        }
                        Meaning::Error
                    }
                }
            }
            Meaning::TypeName(ty) => {
                let kind = SemanticErrorKind::NotCallable {
                    type_name: self.describe(&ty),
                };
                self.error(kind, span.clone());
                Meaning::Error
            }
            Meaning::Namespace(_) => {
                self.error(SemanticErrorKind::NamespaceUsedAsValue, span.clone());
                Meaning::Error
            }
            Meaning::Error => {
                // still check the arguments so their sub-errors surface
                self.check_arguments(arguments);
                Meaning::Error
            }
        }
    }

    pub(super) fn check_arguments(
        &mut self,
        arguments: &'ast [Argument<'ast, 'ast>],
    ) -> Vec<CallArgument<'ast>> {
        arguments
            .iter()
            .map(|argument| self.check_argument_expression(argument))
            .collect()
    }

    fn check_argument_expression(
        &mut self,
        argument: &'ast Argument<'ast, 'ast>,
    ) -> CallArgument<'ast> {
        match &argument.value {
            ArgumentValue::Expression(expression) => {
                if let Expression::Lambda(lambda) = expression {
                    return CallArgument {
                        shape: ArgumentShape::Lambda(lambda),
                        name: argument.name.as_ref().map(|name| name.value),
                        expression: Some(expression),
                        modifier: argument.modifier.as_ref().map(|modifier| modifier.value),
                        is_integer_literal: false,
                        out_declaration: None,
                        is_receiver: false,
                        span: argument.span.clone(),
                    };
                }
                CallArgument {
                    shape: ArgumentShape::Value(self.check_expression(expression)),
                    name: argument.name.as_ref().map(|name| name.value),
                    expression: Some(expression),
                    modifier: argument.modifier.as_ref().map(|modifier| modifier.value),
                    is_integer_literal: Self::is_integer_literal(expression),
                    out_declaration: None,
                    is_receiver: false,
                    span: argument.span.clone(),
                }
            }
            ArgumentValue::Declaration {
                variable_type,
                name,
                ..
            } => {
                let declared = self.resolve_type(variable_type);
                let infer = matches!(declared, Type::Infer);
                if !infer {
                    self.declare_local(name.value, declared.clone());
                }
                CallArgument {
                    shape: ArgumentShape::Value(if infer { Type::Infer } else { declared }),
                    name: argument.name.as_ref().map(|name| name.value),
                    expression: None,
                    modifier: Some(ArgumentModifier::Out),
                    is_integer_literal: false,
                    out_declaration: Some((name.value, infer)),
                    is_receiver: false,
                    span: argument.span.clone(),
                }
            }
            ArgumentValue::Missing => CallArgument {
                shape: ArgumentShape::Value(Type::Error),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                is_receiver: false,
                span: argument.span.clone(),
            },
        }
    }

    /// Overload resolution over a method group, §12.6.3 inference included; when
    /// instance candidates fail, extension methods in scope get their turn with the
    /// receiver as first argument (C# §12.8.10.3).
    pub(super) fn resolve_call(
        &mut self,
        group: MethodGroup<'ast>,
        arguments: Vec<CallArgument<'ast>>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Type {
        let instance_failure = match self.attempt_call(&group, &arguments) {
            AttemptOutcome::Selected(selected) => {
                self.record_call(node, &group, &selected, false);
                return self.finish_call(&selected, &arguments);
            }
            AttemptOutcome::Ambiguous => {
                self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                return Type::Error;
            }
            AttemptOutcome::NoMatch { inference_failed } => inference_failed,
        };

        if group.allow_extensions
            && let Some(receiver) = group.receiver.clone()
            && let Some(extension_group) = self.extension_group(&group)
        {
            let mut extension_arguments = Vec::with_capacity(arguments.len() + 1);
            extension_arguments.push(CallArgument {
                shape: ArgumentShape::Value(receiver),
                name: None,
                expression: None,
                modifier: None,
                is_integer_literal: false,
                out_declaration: None,
                is_receiver: true,
                span: span.clone(),
            });
            extension_arguments.extend(arguments);

            match self.attempt_call(&extension_group, &extension_arguments) {
                AttemptOutcome::Selected(selected) => {
                    self.record_call(node, &extension_group, &selected, true);
                    return self.finish_call(&selected, &extension_arguments);
                }
                AttemptOutcome::Ambiguous => {
                    self.error(SemanticErrorKind::AmbiguousOverload, span.clone());
                    return Type::Error;
                }
                AttemptOutcome::NoMatch { inference_failed } => {
                    // extension candidates existed, so this is a mismatch,
                    // not an unknown member
                    self.report_call_failure(
                        &extension_group,
                        &extension_arguments[1..],
                        instance_failure || inference_failed,
                        span,
                    );
                    return Type::Error;
                }
            }
        }

        self.report_call_failure(&group, &arguments, instance_failure, span);
        Type::Error
    }

    /// Remembers which overload a call node bound to, for the code generator.
    pub(super) fn record_call(
        &mut self,
        node: Option<EntityID>,
        group: &MethodGroup<'ast>,
        selected: &SelectedOverload,
        is_extension: bool,
    ) {
        let Some(node) = node else {
            return;
        };
        let call = self.resolved_call_of(group, selected, is_extension);
        self.targets.insert(node, ResolvedTarget::Call(call));
    }

    pub(super) fn resolved_call_of(
        &self,
        group: &MethodGroup<'ast>,
        selected: &SelectedOverload,
        is_extension: bool,
    ) -> ResolvedCall {
        let candidate = &group.candidates[selected.candidate];
        ResolvedCall {
            origin: candidate.origin.clone(),
            is_static: candidate.is_static,
            is_extension,
            declaring_type: candidate.declaring_type.clone(),
            signature: selected.signature.clone(),
            type_arguments: selected.type_arguments.clone(),
            parameter_of_argument: selected.parameter_of_argument.clone(),
            params_expansion: selected.params_expansion,
        }
    }

    fn report_call_failure(
        &mut self,
        group: &MethodGroup<'ast>,
        arguments: &[CallArgument<'ast>],
        inference_failed: bool,
        span: &Range<usize>,
    ) {
        let kind = if group.candidates.is_empty() {
            SemanticErrorKind::UnknownMember {
                type_name: group.receiver_display.clone(),
            }
        } else if inference_failed {
            SemanticErrorKind::CannotInferTypeArguments
        } else {
            SemanticErrorKind::NoMatchingOverload
        };
        self.error(kind, span.clone());

        for argument in arguments {
            if let (ArgumentShape::Lambda(_), Some(expression)) =
                (&argument.shape, argument.expression)
            {
                self.expression_types
                    .insert(EntityID::from(expression), Type::Error);
            }
        }
    }

    /// One pass of candidate filtering and betterness. Side-effect free apart from
    /// rolled-back lambda probes.
    /// Which parameter each written argument binds to (§12.6.4.2): a
    /// positional argument takes its own position, a named one the parameter
    /// of that name. `None` when the shape does not fit this candidate — an
    /// unknown name, a parameter given twice, or a positional argument after
    /// a named one that moved out of place (C# 7.2 allows positional
    /// arguments after named ones only while the named ones sit where they
    /// would have anyway).
    fn bind_argument_names(
        arguments: &[CallArgument<'ast>],
        parameters: &[crate::types::ParameterSignature],
    ) -> Option<Vec<usize>> {
        let mut used = vec![false; parameters.len()];
        let mut bound = Vec::with_capacity(arguments.len());
        let mut names_moved = false;
        for (position, argument) in arguments.iter().enumerate() {
            let index = match argument.name {
                None => {
                    if names_moved {
                        return None;
                    }
                    position
                }
                Some(name) => {
                    let index = parameters
                        .iter()
                        .position(|parameter| parameter.name.as_deref() == Some(name))?;
                    if index != position {
                        names_moved = true;
                    }
                    index
                }
            };
            if index >= used.len() || used[index] {
                return None;
            }
            used[index] = true;
            bound.push(index);
        }
        Some(bound)
    }

    pub(super) fn attempt_call(
        &mut self,
        group: &MethodGroup<'ast>,
        arguments: &[CallArgument<'ast>],
    ) -> AttemptOutcome {
        let mut viable: Vec<Applicable> = Vec::new();
        let mut inference_failed = false;

        for (candidate_index, candidate) in group.candidates.iter().enumerate() {
            // normal form first; a candidate applicable that way is not also
            // considered in its expanded form (§12.6.4.2)
            match self.try_candidate(group, candidate, candidate_index, arguments, false) {
                Ok(applicable) => {
                    viable.push(applicable);
                    continue;
                }
                Err(failed) => inference_failed |= failed,
            }
            let takes_params = matches!(
                &candidate.signature,
                Some(MemberSignature::Function(function))
                    if function.parameters.last().is_some_and(|parameter| parameter.is_params)
            );
            if takes_params {
                match self.try_candidate(group, candidate, candidate_index, arguments, true) {
                    Ok(applicable) => viable.push(applicable),
                    Err(failed) => inference_failed |= failed,
                }
            }
        }

        // most exact matches wins; then a non-generic method over a generic
        // one (§12.6.4.5: `Min(IEnumerable<int>)` beats `Min<T>(IEnumerable<T>)`
        // once T is int); then normal form over expanded; then the one that
        // fills in the fewest defaults (§12.6.4.3); then the more specific
        // declared parameter types — the fewer type parameters they mention,
        // the more specific (`Max<T>(…, Func<T, int>)` over
        // `Max<T, R>(…, Func<T, R>)` when both fit)
        let rank = |applicable: &Applicable| {
            let candidate = &group.candidates[applicable.selected.candidate];
            let type_parameter_leaves = match &candidate.signature {
                Some(MemberSignature::Function(declared)) => declared
                    .parameters
                    .iter()
                    .map(|parameter| type_parameter_leaves(&parameter.parameter_type))
                    .sum::<usize>(),
                _ => 0,
            };
            (
                applicable.exact,
                applicable.selected.type_arguments.is_empty(),
                !applicable.expanded,
                usize::MAX - applicable.omitted,
                usize::MAX - type_parameter_leaves,
            )
        };
        let Some(best) = viable.iter().map(rank).max() else {
            return AttemptOutcome::NoMatch { inference_failed };
        };
        let mut winners = viable
            .into_iter()
            .filter(|applicable| rank(applicable) == best);
        let selected = winners.next().unwrap().selected;
        if winners.next().is_some() {
            return AttemptOutcome::Ambiguous;
        }
        AttemptOutcome::Selected(selected)
    }

    /// One candidate against the arguments, in normal form or — with the
    /// trailing `params T[]` opened up into one `T` parameter per remaining
    /// argument — in expanded form. `Err(true)` when type inference is what
    /// failed (the diagnostic differs), `Err(false)` otherwise.
    fn try_candidate(
        &mut self,
        group: &MethodGroup<'ast>,
        candidate: &MemberCandidate,
        candidate_index: usize,
        arguments: &[CallArgument<'ast>],
        expanded: bool,
    ) -> Result<Applicable, bool> {
        use crate::types::ParameterSignature;
        let Some(MemberSignature::Function(declared)) = &candidate.signature else {
            return Err(false);
        };
        if group.via_type && !candidate.is_static {
            return Err(false);
        }

        let fixed = declared.parameters.len().saturating_sub(1);
        let working: FunctionSignature = if expanded {
            let Some(last) = declared
                .parameters
                .last()
                .filter(|parameter| parameter.is_params)
            else {
                return Err(false);
            };
            let Type::Array { element, rank: 1 } = &last.parameter_type else {
                return Err(false);
            };
            let mut parameters = declared.parameters[..fixed].to_vec();
            for _ in 0..arguments.len().saturating_sub(fixed) {
                parameters.push(ParameterSignature {
                    passing: ParameterPassing::Value,
                    is_params: false,
                    parameter_type: (**element).clone(),
                    name: None,
                    default_value: None,
                });
            }
            FunctionSignature {
                return_type: declared.return_type.clone(),
                parameters,
            }
        } else {
            declared.clone()
        };

        if arguments.len() > working.parameters.len() {
            return Err(false);
        }
        // named arguments pick their parameter; the rest go by position
        let Some(parameter_of_argument) = Self::bind_argument_names(arguments, &working.parameters)
        else {
            return Err(false);
        };
        // whatever is left unbound must be optional (§12.6.4.2); a `params`
        // parameter left out is the expanded form's business, not a default
        let omitted = working
            .parameters
            .iter()
            .enumerate()
            .filter(|(index, _)| !parameter_of_argument.contains(index))
            .count();
        if working
            .parameters
            .iter()
            .enumerate()
            .any(|(index, parameter)| {
                !parameter_of_argument.contains(&index) && parameter.default_value.is_none()
            })
        {
            return Err(false);
        }
        let pairs: Vec<(usize, usize)> =
            parameter_of_argument.iter().copied().enumerate().collect();

        // the method's own generic parameters: explicit, inferred, or absent
        let (working, declared, type_arguments) = if candidate.arity > 0 {
            let keys = method_parameter_keys(&self.system(), candidate);
            let mut engine = Inference::new(keys.clone());

            if !group.explicit_arguments.is_empty() {
                if group.explicit_arguments.len() != keys.len() {
                    return Err(false);
                }
                for (key, ty) in keys.iter().zip(&group.explicit_arguments) {
                    engine.preset(*key, ty.clone());
                }
            } else {
                // phase 1: ordinary arguments contribute bounds
                for &(argument_index, parameter_index) in &pairs {
                    let argument = &arguments[argument_index];
                    let parameter = &working.parameters[parameter_index];
                    if let ArgumentShape::Value(ty) = &argument.shape {
                        let system = self.system();
                        engine.lower_bound(&system, &parameter.parameter_type, ty);
                    }
                }
                {
                    let system = self.system();
                    engine.fix_where_possible(&system);
                }

                // phase 2: lambda bodies, typed against now-concrete inputs,
                // feed their return types back — round after round, since
                // what one lambda returns can be what the next one takes
                // (`SelectMany(s => s.Items, (s, item) => ...)`): a lambda
                // waits until every input of its delegate is fixed
                let mut pending: Vec<(usize, usize)> = pairs
                    .iter()
                    .copied()
                    .filter(|&(argument_index, _)| {
                        matches!(arguments[argument_index].shape, ArgumentShape::Lambda(_))
                    })
                    .collect();
                loop {
                    let mut progressed = false;
                    let mut waiting: Vec<(usize, usize)> = Vec::new();
                    for (argument_index, parameter_index) in pending {
                        let argument = &arguments[argument_index];
                        let parameter = &working.parameters[parameter_index];
                        let ArgumentShape::Lambda(lambda) = &argument.shape else {
                            continue;
                        };
                        let parameter_type = engine.substitute(&parameter.parameter_type);
                        let Some(delegate) = self.delegate_signature(&parameter_type) else {
                            return Err(false);
                        };
                        if delegate
                            .parameters
                            .iter()
                            .any(|parameter| engine.has_unfixed(&parameter.parameter_type))
                        {
                            waiting.push((argument_index, parameter_index));
                            continue;
                        }
                        if !Self::lambda_shape_matches(lambda, &delegate) {
                            return Err(false);
                        }
                        let Some(returned) = self.probe_lambda_return(lambda, &delegate) else {
                            return Err(false);
                        };
                        let system = self.system();
                        engine.lower_bound(&system, &delegate.return_type, &returned);
                        progressed = true;
                    }
                    {
                        let system = self.system();
                        engine.fix_where_possible(&system);
                    }
                    if waiting.is_empty() {
                        break;
                    }
                    if !progressed {
                        // a lambda whose inputs nothing can fix
                        return Err(true);
                    }
                    pending = waiting;
                }
                if !engine.all_fixed() {
                    return Err(true);
                }
            }

            let type_arguments: Vec<Type> = keys
                .iter()
                .map(|key| {
                    engine.substitute(&match key {
                        InferenceKey::Source(symbol) => Type::TypeParameter(*symbol),
                        InferenceKey::External(index) => Type::ExternalMethodTypeParameter(*index),
                    })
                })
                .collect();
            let substitute = |signature: &FunctionSignature| match engine
                .substitute_signature(&MemberSignature::Function(signature.clone()))
            {
                MemberSignature::Function(function) => function,
                _ => unreachable!(),
            };
            (substitute(&working), substitute(declared), type_arguments)
        } else {
            if !group.explicit_arguments.is_empty() {
                return Err(false);
            }
            (working, declared.clone(), Vec::new())
        };

        // applicability
        let mut exact = 0usize;
        for &(argument_index, parameter_index) in &pairs {
            let argument = &arguments[argument_index];
            let parameter = &working.parameters[parameter_index];
            let modifier_ok = match parameter.passing {
                ParameterPassing::Ref => argument.modifier == Some(ArgumentModifier::Ref),
                ParameterPassing::Out => argument.modifier == Some(ArgumentModifier::Out),
                ParameterPassing::In => argument
                    .modifier
                    .map(|modifier| modifier == ArgumentModifier::In)
                    .unwrap_or(true),
                ParameterPassing::Value => argument.modifier.is_none(),
            };
            if !modifier_ok {
                return Err(false);
            }

            match &argument.shape {
                ArgumentShape::Value(ty) => {
                    if argument
                        .out_declaration
                        .map(|(_, infer)| infer)
                        .unwrap_or(false)
                    {
                        // `out var x` matches any out parameter
                        continue;
                    }
                    if *ty == parameter.parameter_type {
                        exact += 1;
                        continue;
                    }
                    // `ref`/`out` write through the reference, so the types
                    // must be identical — a conversion would leave the
                    // callee writing into a slot of the wrong type (§12.6.4.2)
                    if matches!(
                        parameter.passing,
                        ParameterPassing::Ref | ParameterPassing::Out
                    ) && !matches!(ty, Type::Error)
                    {
                        return Err(false);
                    }
                    let system = self.system();
                    let convertible = if argument.is_receiver {
                        system.is_standard_implicit_conversion(ty, &parameter.parameter_type)
                    } else {
                        system.is_implicitly_convertible(ty, &parameter.parameter_type)
                    } || (argument.is_integer_literal
                        && system
                            .numeric_kind(&parameter.parameter_type)
                            .map(|kind| kind.is_integral())
                            .unwrap_or(false));
                    if !convertible {
                        return Err(false);
                    }
                }
                ArgumentShape::Lambda(lambda) => {
                    let Some(delegate) = self.delegate_signature(&parameter.parameter_type) else {
                        return Err(false);
                    };
                    if !Self::lambda_shape_matches(lambda, &delegate) {
                        return Err(false);
                    }
                    // a body that already has the delegate's return type is
                    // the better conversion (§12.6.4.5): `Sum(x => x.Price)`
                    // with an int `Price` is the `Func<T, int>` overload,
                    // not the long, float or double ones it also fits
                    if delegate.return_type != Type::Void
                        && self.probe_lambda_return(lambda, &delegate).as_ref()
                            == Some(&delegate.return_type)
                    {
                        exact += 1;
                    }
                }
            }
        }

        Ok(Applicable {
            selected: SelectedOverload {
                signature: declared,
                candidate: candidate_index,
                type_arguments,
                parameter_of_argument,
                params_expansion: expanded.then_some(fixed),
            },
            exact,
            omitted,
            expanded,
        })
    }

    /// The chosen overload's side effects: lambda bodies checked for real,
    /// `out var` locals bound, argument expressions typed.
    pub(super) fn finish_call(
        &mut self,
        selected: &SelectedOverload,
        arguments: &[CallArgument<'ast>],
    ) -> Type {
        let signature = &selected.signature;
        for (argument_index, argument) in arguments.iter().enumerate() {
            let parameter_index = selected
                .parameter_of_argument
                .get(argument_index)
                .copied()
                .unwrap_or(argument_index);
            let expanded_element = selected
                .params_expansion
                .filter(|&fixed| parameter_index >= fixed)
                .and_then(|fixed| signature.parameters.get(fixed))
                .and_then(|parameter| match &parameter.parameter_type {
                    Type::Array { element, .. } => Some((**element).clone()),
                    _ => None,
                });
            let element_parameter;
            let parameter = match expanded_element {
                Some(element) => {
                    element_parameter = crate::types::ParameterSignature {
                        passing: crate::types::ParameterPassing::Value,
                        is_params: false,
                        parameter_type: element,
                        name: None,
                        default_value: None,
                    };
                    &element_parameter
                }
                None => match signature.parameters.get(parameter_index) {
                    Some(parameter) => parameter,
                    None => continue,
                },
            };
            match &argument.shape {
                ArgumentShape::Lambda(lambda) => {
                    if let Some(delegate) = self.delegate_signature(&parameter.parameter_type) {
                        self.check_lambda_against(lambda, &delegate);
                    }
                    if let Some(expression) = argument.expression {
                        self.expression_types
                            .insert(EntityID::from(expression), parameter.parameter_type.clone());
                    }
                }
                ArgumentShape::Value(_) => {
                    if let Some((name, true)) = argument.out_declaration {
                        self.declare_local(name, parameter.parameter_type.clone());
                    }
                }
            }
        }
        signature.return_type.clone()
    }

    /// The extension methods named like this group's member, gathered from every
    /// namespace scope and `using` import, nearest scope first.
    /// A static member of a type brought in by `using static T;`, named
    /// bare: `Ok(value)` for `using static MenSharp.Result;`. Every such
    /// type in scope contributes its members named `name`; the group then
    /// resolves as a call through the type, so an instance member never
    /// applies and two types offering the same name are an ambiguity, as
    /// in C#.
    pub(super) fn static_using_meaning(
        &mut self,
        name: &'ast str,
        explicit_arguments: Vec<Type>,
        span: &Range<usize>,
        node: Option<EntityID>,
    ) -> Option<Meaning<'ast>> {
        let mut imported: Vec<Type> = Vec::new();
        for scope in self.scopes.iter().rev() {
            for using in &scope.usings {
                if let crate::semantics::resolve::ResolvedUsing::Static(Resolution::Type {
                    target,
                    arguments,
                }) = using
                {
                    imported.push(Type::Named {
                        target: *target,
                        arguments: arguments.clone(),
                    });
                }
            }
        }
        let mut candidates: Vec<MemberCandidate> = Vec::new();
        let mut owner: Option<Type> = None;
        for ty in imported {
            let found: Vec<MemberCandidate> = self
                .system()
                .members_named(&ty, name)
                .into_iter()
                .filter(|candidate| candidate.is_static && !candidate.kind.is_type())
                .collect();
            if !found.is_empty() {
                owner.get_or_insert(ty);
                candidates.extend(found);
            }
        }
        let owner = owner?;
        self.member_meaning(
            candidates,
            AccessContext {
                receiver: Some(owner),
                via_type: true,
                implicit_this: false,
            },
            explicit_arguments,
            name,
            span,
            node,
        )
    }

    pub(super) fn extension_group(&self, group: &MethodGroup<'ast>) -> Option<MethodGroup<'ast>> {
        let name = group.name;

        let mut search: Vec<(Vec<&'ast str>, Option<SymbolId>)> = Vec::new();
        for scope in self.scopes.iter().rev() {
            search.push((scope.path.clone(), scope.symbol));
            for using in &scope.usings {
                if let crate::semantics::resolve::ResolvedUsing::Namespace(path) = using {
                    let symbol = self.resolver.source_namespace_at(path);
                    search.push((path.clone(), symbol));
                }
            }
        }

        let mut candidates: Vec<MemberCandidate> = Vec::new();
        for (path, namespace_symbol) in &search {
            // source static classes declared in this namespace
            if let Some(namespace_symbol) = namespace_symbol {
                for &class in &self
                    .resolver
                    .declarations
                    .table
                    .symbol(*namespace_symbol)
                    .members
                {
                    if self.resolver.declarations.table.symbol(class).kind != SymbolKind::Class {
                        continue;
                    }
                    let class_type = Type::Named {
                        target: TypeTarget::Source(class),
                        arguments: Vec::new(),
                    };
                    for candidate in self.system().members_named(&class_type, name) {
                        let is_extension = match &candidate.origin {
                            MemberOrigin::Source(id) => {
                                self.resolver.declarations.table.symbol(*id).is_extension
                            }
                            MemberOrigin::External { member, .. } => member.is_extension,
                            MemberOrigin::LocalFunction(_) => false,
                        };
                        if is_extension && candidate.declaring_type == class_type {
                            candidates.push(candidate);
                        }
                    }
                }
            }

            // external static classes with matching extension methods — unless
            // a source class of the same name shadows the whole class, the way
            // it shadows the type (CS0436): the corlib's `System.Linq.Enumerable`
            // stands in for the reference assembly's, extension methods included
            for owner in self.resolver.external.extension_method_owners(path, name) {
                let display = self.resolver.external.display_name(owner);
                let simple = display.rsplit('.').next().unwrap_or(&display);
                let shadowed = namespace_symbol.is_some_and(|namespace_symbol| {
                    self.resolver
                        .declarations
                        .table
                        .symbol(namespace_symbol)
                        .members
                        .iter()
                        .any(|&member| {
                            let entry = self.resolver.declarations.table.symbol(member);
                            entry.kind == SymbolKind::Class && entry.name == simple
                        })
                });
                if shadowed {
                    continue;
                }
                let owner_type = Type::Named {
                    target: TypeTarget::External(owner),
                    arguments: Vec::new(),
                };
                for candidate in self.system().members_named(&owner_type, name) {
                    let is_extension = matches!(
                        &candidate.origin,
                        MemberOrigin::External { member, .. } if member.is_extension
                    );
                    if is_extension && candidate.declaring_type == owner_type {
                        candidates.push(candidate);
                    }
                }
            }
        }

        if candidates.is_empty() {
            return None;
        }
        Some(MethodGroup {
            candidates,
            explicit_arguments: group.explicit_arguments.clone(),
            via_type: true,
            name,
            receiver: None,
            allow_extensions: false,
            receiver_display: group.receiver_display.clone(),
            span: group.span.clone(),
        })
    }
}
