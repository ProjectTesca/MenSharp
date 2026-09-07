//! Bodies that suspend or repeat: iterators, `async`/`await`, `foreach`.

use super::{
    AttemptOutcome, Checker, ForeachEnumeration, MethodGroup, ResolvedAwait, ResolvedCall,
    ResolvedMember,
};
use crate::error::SemanticErrorKind;
use crate::symbol::{SymbolId, SymbolKind};
use crate::types::lookup::{MemberCandidate, MemberOrigin, TypeSystem};
use crate::types::{FunctionSignature, MemberSignature, ParameterPassing, Type, TypeTarget};
use men_sharp_parser::ast::{
    AwaitExpression, EntityID, LambdaExpression, LambdaParameters, Modifier,
};
use std::ops::Range;

impl<'a, 'ast> Checker<'a, 'ast> {
    /// Opens a body: `yield` is allowed inside when `declared` is
    /// `IEnumerable<T>`/`IEnumerator<T>`. Returns what to hand back to
    /// `leave_iterator_body`.
    pub(super) fn enter_iterator_body(&mut self, declared: &Type) -> (Option<Type>, bool, bool) {
        let element = self.iterator_element_of(declared);
        (
            std::mem::replace(&mut self.iterator_element, element),
            std::mem::replace(&mut self.yield_seen, false),
            std::mem::replace(&mut self.value_return_seen, false),
        )
    }

    /// Closes a body: records it as an iterator when it yielded, and
    /// rejects `return value;` alongside `yield` (CS1622).
    pub(super) fn leave_iterator_body(
        &mut self,
        saved: (Option<Type>, bool, bool),
        node: EntityID,
        span: Range<usize>,
    ) {
        if self.yield_seen {
            self.iterators.insert(node);
            if self.value_return_seen {
                self.error(SemanticErrorKind::ReturnInIterator, span);
            }
        }
        self.iterator_element = saved.0;
        self.yield_seen = saved.1;
        self.value_return_seen = saved.2;
    }

    /// `T` when `declared` is the mini-corlib's `IEnumerable<T>` or
    /// `IEnumerator<T>` — the types an iterator may be declared with.
    fn iterator_element_of(&self, declared: &Type) -> Option<Type> {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            arguments,
        } = declared
        else {
            return None;
        };
        let path = self.symbol_path_of(*symbol);
        let is_enumerable = path == ["System", "Collections", "Generic", "IEnumerable"]
            || path == ["System", "Collections", "Generic", "IEnumerator"];
        if is_enumerable && arguments.len() == 1 {
            Some(arguments[0].clone())
        } else {
            None
        }
    }

    pub(super) fn check_yield(
        &mut self,
        statement: &'ast men_sharp_parser::ast::YieldStatement<'ast, 'ast>,
    ) {
        use men_sharp_parser::ast::YieldKind;
        let Some(element) = self.iterator_element.clone() else {
            if let Some(value) = &statement.value {
                self.check_expression(value);
            }
            self.error(
                SemanticErrorKind::YieldOutsideIterator,
                statement.yield_keyword.clone(),
            );
            return;
        };
        // §13.15: `yield` of either kind is out in a `finally`; a `yield
        // return` is also out in a `catch`, and in a `try` that has one
        let returns = statement.kind.value == YieldKind::Return;
        let region = if self.finally_depth > 0 {
            Some("a `finally` block")
        } else if returns && self.catch_depth_for_yield > 0 {
            Some("a `catch` block")
        } else if returns && self.guarded_depth > 0 {
            Some("a `try` block that has a `catch` (only `try`/`finally` may yield)")
        } else {
            None
        };
        if let Some(region) = region {
            let kind = SemanticErrorKind::YieldInsideTry {
                region: region.to_string(),
            };
            self.error(kind, statement.yield_keyword.clone());
        }
        self.yield_seen = true;
        match (statement.kind.value, &statement.value) {
            (YieldKind::Return, Some(value)) => {
                let literal = Self::is_integer_literal(value);
                let ty = self.check_expression_expecting(value, Some(&element));
                self.require_convertible(&ty, &element, literal, value.span());
            }
            (YieldKind::Return, None) => {
                self.error(
                    SemanticErrorKind::ReturnValueMismatch,
                    statement.span.clone(),
                );
            }
            (YieldKind::Break, Some(value)) => {
                self.check_expression(value);
                self.error(
                    SemanticErrorKind::ReturnValueMismatch,
                    statement.span.clone(),
                );
            }
            (YieldKind::Break, None) => {}
        }
    }

    // ------------------------------------------------------------ async

    pub(super) fn is_async_lambda(lambda: &LambdaExpression<'ast, 'ast>) -> bool {
        lambda
            .modifiers
            .iter()
            .any(|modifier| modifier.value == Modifier::Async)
    }

    /// The declared symbol's namespace-and-name path, root first.
    fn symbol_path_of(&self, symbol: SymbolId) -> Vec<&'ast str> {
        let table = &self.resolver.declarations.table;
        let mut parts = Vec::new();
        let mut current = Some(symbol);
        while let Some(id) = current {
            let entry = table.symbol(id);
            if !entry.name.is_empty() {
                parts.push(entry.name);
            }
            current = entry.parent;
        }
        parts.reverse();
        parts
    }

    /// Is this the mini-corlib's `System.Threading.Tasks.Task` (either arity)?
    fn is_task_symbol(&self, symbol: SymbolId) -> bool {
        self.symbol_path_of(symbol) == ["System", "Threading", "Tasks", "Task"]
    }

    /// `Task` for `void`, `Task<T>` otherwise.
    pub(super) fn task_type(&self, inner: &Type) -> Option<Type> {
        let table = &self.resolver.declarations.table;
        let mut current = table.root();
        for segment in ["System", "Threading", "Tasks"] {
            current = *table.symbol(current).members_named(segment).first()?;
        }
        let wanted = usize::from(*inner != Type::Void);
        let symbol = table
            .symbol(current)
            .members_named("Task")
            .iter()
            .copied()
            .find(|candidate| table.symbol(*candidate).type_parameters.len() == wanted)?;
        Some(Type::Named {
            target: TypeTarget::Source(symbol),
            arguments: if wanted == 1 {
                vec![inner.clone()]
            } else {
                Vec::new()
            },
        })
    }

    /// What `return` in an `async` body with this declared return type
    /// hands its task: `void` and `Task` → nothing, `Task<T>` → `T`.
    fn async_inner(&self, declared: &Type) -> Option<Type> {
        match declared {
            Type::Void | Type::Error => Some(Type::Void),
            Type::Named {
                target: TypeTarget::Source(symbol),
                arguments,
            } if self.is_task_symbol(*symbol) => {
                Some(arguments.first().cloned().unwrap_or(Type::Void))
            }
            _ => None,
        }
    }

    /// The signature an `async` body is checked against: the declared one
    /// with its return type unwrapped — or `None`, with the errors
    /// reported, when the declaration is not a valid async one.
    pub(super) fn async_signature(
        &mut self,
        declared: &FunctionSignature,
        span: Range<usize>,
    ) -> Option<FunctionSignature> {
        let Some(inner) = self.async_inner(&declared.return_type) else {
            let kind = SemanticErrorKind::AsyncReturnType {
                type_name: self.describe(&declared.return_type),
            };
            self.error(kind, span);
            return None;
        };
        if declared
            .parameters
            .iter()
            .any(|parameter| parameter.passing != ParameterPassing::Value)
        {
            self.error(SemanticErrorKind::AsyncByRefParameter, span);
            return None;
        }
        Some(FunctionSignature {
            return_type: inner,
            parameters: declared.parameters.clone(),
        })
    }

    /// `await e`: `e` must be awaitable by the awaiter pattern, and the
    /// expression is what `GetResult()` returns.
    pub(super) fn check_await(&mut self, node: &'ast AwaitExpression<'ast, 'ast>) -> Type {
        let Ok(value) = &node.value else {
            return Type::Error;
        };
        let ty = self.check_expression(value);
        if !self.in_async {
            let hint = self.async_hint();
            self.error_with_hint(
                SemanticErrorKind::AwaitOutsideAsync,
                node.await_keyword.clone(),
                hint,
            );
            return Type::Error;
        }
        if matches!(ty, Type::Error) {
            return Type::Error;
        }
        match self.resolve_await(&ty) {
            Some(resolved) => {
                let result = resolved.result_type.clone();
                self.awaits.insert(EntityID::from(node), resolved);
                result
            }
            None => {
                let kind = SemanticErrorKind::NotAwaitable {
                    type_name: self.describe(&ty),
                };
                self.error(kind, value.span());
                Type::Error
            }
        }
    }

    /// The awaiter pattern on `awaitable`, bound to source members: an
    /// instance `GetAwaiter()` whose result has a `bool IsCompleted`,
    /// `void OnCompleted(Action)` and `GetResult()`.
    fn resolve_await(&self, awaitable: &Type) -> Option<ResolvedAwait> {
        let system = self.system();
        let get_awaiter = self.instance_method(&system, awaitable, "GetAwaiter", 0)?;
        let awaiter_type = get_awaiter.signature.return_type.clone();
        let boolean = self.corlib("Boolean");
        let is_completed = system
            .members_named(&awaiter_type, "IsCompleted")
            .into_iter()
            .find_map(|candidate| {
                if candidate.is_static
                    || candidate.kind != SymbolKind::Property
                    || !matches!(candidate.origin, MemberOrigin::Source(_))
                {
                    return None;
                }
                match &candidate.signature {
                    Some(MemberSignature::Property(ty)) if *ty == boolean => Some(ResolvedMember {
                        origin: candidate.origin.clone(),
                        kind: candidate.kind,
                        is_static: false,
                        declaring_type: candidate.declaring_type.clone(),
                        member_type: ty.clone(),
                    }),
                    _ => None,
                }
            })?;
        let on_completed = self.instance_method(&system, &awaiter_type, "OnCompleted", 1)?;
        if on_completed.signature.parameters[0].parameter_type != self.corlib("Action")
            || on_completed.signature.return_type != Type::Void
        {
            return None;
        }
        let get_result = self.instance_method(&system, &awaiter_type, "GetResult", 0)?;
        let result_type = get_result.signature.return_type.clone();
        Some(ResolvedAwait {
            get_awaiter,
            awaiter_type,
            is_completed,
            on_completed,
            get_result,
            result_type,
        })
    }

    /// A non-generic instance method of a source type, by name and
    /// parameter count, as a resolved call with its arguments in order.
    fn instance_method(
        &self,
        system: &TypeSystem<'_, 'ast>,
        receiver: &Type,
        name: &str,
        arity: usize,
    ) -> Option<ResolvedCall> {
        system
            .members_named(receiver, name)
            .into_iter()
            .find_map(|candidate| {
                if candidate.is_static
                    || candidate.kind != SymbolKind::Method
                    || candidate.arity != 0
                    || !matches!(candidate.origin, MemberOrigin::Source(_))
                {
                    return None;
                }
                let Some(MemberSignature::Function(signature)) = &candidate.signature else {
                    return None;
                };
                if signature.parameters.len() != arity {
                    return None;
                }
                Some(ResolvedCall {
                    origin: candidate.origin.clone(),
                    is_static: false,
                    is_extension: false,
                    declaring_type: candidate.declaring_type.clone(),
                    signature: signature.clone(),
                    type_arguments: Vec::new(),
                    parameter_of_argument: (0..arity).collect(),
                    params_expansion: None,
                })
            })
    }

    /// A lambda in a non-argument position: against the context's expected type,
    /// or with its C# 10 natural `Func<>`/`Action<>` type.
    pub(super) fn check_lambda(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        expected: Option<&Type>,
    ) -> Type {
        if let Some(expected) = expected {
            if let Some(delegate) = self.delegate_signature(expected) {
                self.check_lambda_against(lambda, &delegate);
                return expected.clone();
            }
            if !matches!(expected, Type::Error) {
                let kind = SemanticErrorKind::TypeMismatch {
                    expected: self.describe(expected),
                    found: "lambda".to_string(),
                };
                self.error(kind, lambda.span.clone());
                return Type::Error;
            }
            return Type::Error;
        }

        // natural type: every parameter must carry a written type
        let Some(parameter_types) = self.lambda_written_parameter_types(lambda) else {
            self.error(SemanticErrorKind::TypeAnnotationNeeded, lambda.span.clone());
            return Type::Error;
        };
        let probe = FunctionSignature {
            return_type: Type::Void,
            parameters: parameter_types
                .iter()
                .map(|ty| crate::types::ParameterSignature {
                    passing: crate::types::ParameterPassing::Value,
                    is_params: false,
                    parameter_type: ty.clone(),
                    name: None,
                    default_value: None,
                })
                .collect(),
        };
        let Some(returned) = self.probe_lambda_return(lambda, &probe) else {
            return Type::Error;
        };
        let Some(delegate_type) = self.func_or_action(&parameter_types, &returned) else {
            self.error(SemanticErrorKind::TypeAnnotationNeeded, lambda.span.clone());
            return Type::Error;
        };

        self.check_lambda_against(
            lambda,
            &FunctionSignature {
                return_type: returned,
                parameters: probe.parameters,
            },
        );
        delegate_type
    }

    /// `Some` only when every parameter has an explicit type (zero parameters
    /// qualifies); `x => ...` has no natural type in C# either.
    fn lambda_written_parameter_types(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
    ) -> Option<Vec<Type>> {
        match &lambda.parameters {
            LambdaParameters::Single(_) => None,
            LambdaParameters::List(list) => {
                let mut types = Vec::with_capacity(list.parameters.len());
                for parameter in list.parameters {
                    let written = parameter.parameter_type.as_ref()?;
                    types.push(self.resolve_type(written));
                }
                Some(types)
            }
        }
    }

    /// `System.Func<..., R>` / `System.Action<...>` for a signature.
    pub(super) fn func_or_action(&self, parameters: &[Type], returned: &Type) -> Option<Type> {
        let (name, arity, arguments) = if *returned == Type::Void {
            ("Action", parameters.len() as u32, parameters.to_vec())
        } else {
            let mut arguments = parameters.to_vec();
            arguments.push(returned.clone());
            ("Func", parameters.len() as u32 + 1, arguments)
        };

        let id = self.resolver.external.find_type(&["System"], name, arity)?;
        Some(Type::Named {
            target: TypeTarget::External(id),
            arguments,
        })
    }

    // -------------------------------------------------------------- foreach

    /// The element type a `foreach` over `collection` yields. Arrays and
    /// strings are walked by index; everything else goes through the
    /// enumerator pattern, which is resolved here and recorded under `node`
    /// for the code generator.
    pub(super) fn element_type_of(
        &mut self,
        collection: &Type,
        span: Range<usize>,
        node: EntityID,
    ) -> Type {
        match collection {
            // any rank: a rectangular array is walked element by element,
            // in row-major order (§13.9.5)
            Type::Array { element, .. } => return (**element).clone(),
            Type::Error | Type::Dynamic => return Type::Error,
            _ if self.system().is_string(collection) => return self.corlib("Char"),
            _ => {}
        }

        // the pattern-based protocol: GetEnumerator(), then MoveNext()/Current
        if let Some(enumeration) = self.resolve_enumeration(collection, &span) {
            let element = enumeration.current.member_type.clone();
            self.enumerations.insert(node, enumeration);
            return element;
        }

        // IEnumerable<T> somewhere in the closure: typed, but with no members
        // for the code generator to call — it reports that itself
        if let Some(ienumerable) = self.resolver.external.find_type(
            &["System", "Collections", "Generic"],
            "IEnumerable",
            1,
        ) {
            let mut visited = std::collections::HashSet::new();
            let mut queue = vec![collection.clone()];
            while let Some(current) = queue.pop() {
                if !visited.insert(current.clone()) {
                    continue;
                }
                if let Type::Named {
                    target: TypeTarget::External(id),
                    arguments,
                } = &current
                    && *id == ienumerable
                    && arguments.len() == 1
                {
                    return arguments[0].clone();
                }
                queue.extend(self.system().interfaces_of(&current));
                if let Some(base) = self.system().base_of(&current) {
                    queue.push(base);
                }
            }
        }

        let kind = SemanticErrorKind::NotEnumerable {
            type_name: self.describe(collection),
        };
        self.error(kind, span);
        Type::Error
    }

    /// Binds the enumerator pattern on `collection`: an accessible instance
    /// `GetEnumerator()` taking nothing, whose result has an instance
    /// `MoveNext()` returning `bool` and a readable `Current` property. Any
    /// piece missing means the pattern does not apply (`None`), not an error —
    /// the caller has the interface fallback and the diagnostic.
    fn resolve_enumeration(
        &mut self,
        collection: &Type,
        span: &Range<usize>,
    ) -> Option<ForeachEnumeration> {
        let (get_enumerator, enumerator_type) =
            self.resolve_parameterless_call(collection, "GetEnumerator", span)?;
        let (move_next, move_next_type) =
            self.resolve_parameterless_call(&enumerator_type, "MoveNext", span)?;
        if !self.system().is_system_type(&move_next_type, "Boolean") {
            return None;
        }
        let current = self
            .system()
            .members_named(&enumerator_type, "Current")
            .into_iter()
            .find(|candidate| candidate.kind == SymbolKind::Property && !candidate.is_static)?;
        let Some(MemberSignature::Property(member_type)) = current.signature.clone() else {
            return None;
        };
        let dispose = self
            .resolve_parameterless_call(&enumerator_type, "Dispose", span)
            .map(|(call, _)| call);
        Some(ForeachEnumeration {
            get_enumerator,
            enumerator_type,
            move_next,
            dispose,
            current: ResolvedMember {
                origin: current.origin,
                kind: current.kind,
                is_static: current.is_static,
                declaring_type: current.declaring_type,
                member_type,
            },
        })
    }

    /// Overload resolution for `receiver.name()` with no arguments and no
    /// syntax to hang an error on: the selected overload and its return type,
    /// or `None` when nothing fits.
    fn resolve_parameterless_call(
        &mut self,
        receiver: &Type,
        name: &'ast str,
        span: &Range<usize>,
    ) -> Option<(ResolvedCall, Type)> {
        let candidates: Vec<MemberCandidate> = self
            .system()
            .members_named(receiver, name)
            .into_iter()
            .filter(|candidate| candidate.kind == SymbolKind::Method && !candidate.is_static)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let group = MethodGroup {
            candidates,
            explicit_arguments: Vec::new(),
            via_type: false,
            name,
            receiver: Some(receiver.clone()),
            allow_extensions: false,
            receiver_display: self.describe(receiver),
            span: span.clone(),
        };
        let AttemptOutcome::Selected(selected) = self.attempt_call(&group, &[]) else {
            return None;
        };
        let candidate = &group.candidates[selected.candidate];
        let call = ResolvedCall {
            origin: candidate.origin.clone(),
            is_static: candidate.is_static,
            is_extension: false,
            declaring_type: candidate.declaring_type.clone(),
            signature: selected.signature.clone(),
            type_arguments: selected.type_arguments.clone(),
            parameter_of_argument: selected.parameter_of_argument.clone(),
            params_expansion: selected.params_expansion,
        };
        let return_type = selected.signature.return_type.clone();
        Some((call, return_type))
    }
}
