//! Lambdas and the delegates they become.

use super::{Checker, MethodGroup, ResolvedCall, ResolvedTarget, Scope};
use crate::error::SemanticErrorKind;
use crate::symbol::SymbolKind;
use crate::types::external::ExternalTypeKind;
use crate::types::infer::best_common_type;
use crate::types::lookup::{MemberCandidate, MemberOrigin};
use crate::types::{FunctionSignature, MemberSignature, Type, TypeTarget};
use men_sharp_parser::ast::{EntityID, LambdaBody, LambdaExpression, LambdaParameters};

impl<'a, 'ast> Checker<'a, 'ast> {
    /// A method group became a delegate: which method, for the code
    /// generator — recorded on the expression node as a call whose
    /// arguments are the delegate's parameters, in order.
    pub(super) fn record_group_conversion(
        &mut self,
        group: &MethodGroup<'ast>,
        index: usize,
        node: Option<EntityID>,
    ) {
        let Some(node) = node else {
            return;
        };
        let candidate = &group.candidates[index];
        let Some(MemberSignature::Function(signature)) = candidate.signature.clone() else {
            return;
        };
        let call = ResolvedCall {
            origin: candidate.origin.clone(),
            is_static: candidate.is_static,
            is_extension: false,
            declaring_type: candidate.declaring_type.clone(),
            parameter_of_argument: (0..signature.parameters.len()).collect(),
            signature,
            type_arguments: group.explicit_arguments.clone(),
            params_expansion: None,
        };
        self.targets.insert(node, ResolvedTarget::Call(call));
    }

    /// `Invoke` on a delegate type of the compilation's own: the delegate
    /// declares no members, its signature *is* the method.
    pub(super) fn source_delegate_invoke(&self, ty: &Type) -> Option<MemberCandidate> {
        let Type::Named {
            target: TypeTarget::Source(symbol),
            ..
        } = ty
        else {
            return None;
        };
        let signature = self.delegate_signature(ty)?;
        Some(MemberCandidate {
            origin: MemberOrigin::Source(*symbol),
            kind: SymbolKind::Method,
            is_static: false,
            accessibility: crate::symbol::Accessibility::Public,
            arity: 0,
            signature: Some(MemberSignature::Function(signature)),
            declaring_type: ty.clone(),
        })
    }

    /// The `Invoke` shape of a delegate type: what a lambda checks against.
    pub(super) fn delegate_signature(&self, ty: &Type) -> Option<FunctionSignature> {
        match ty {
            Type::Named {
                target: TypeTarget::Source(symbol),
                arguments,
            } => {
                if self.resolver.declarations.table.symbol(*symbol).kind != SymbolKind::Delegate {
                    return None;
                }
                let MemberSignature::Function(function) =
                    self.signatures.members.get(symbol)?.clone()
                else {
                    return None;
                };
                let system = self.system();
                match system.instantiate_signature(
                    &MemberSignature::Function(function),
                    &TypeTarget::Source(*symbol),
                    arguments,
                ) {
                    MemberSignature::Function(function) => Some(function),
                    _ => None,
                }
            }
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => {
                if self.resolver.external.type_info(*id).kind != ExternalTypeKind::Delegate {
                    return None;
                }
                self.system()
                    .members_named(ty, "Invoke")
                    .into_iter()
                    .find_map(|candidate| match candidate.signature {
                        Some(MemberSignature::Function(function)) => Some(function),
                        _ => None,
                    })
            }
            _ => None,
        }
    }

    fn lambda_parameter_names(
        lambda: &'ast LambdaExpression<'ast, 'ast>,
    ) -> Vec<Option<&'ast str>> {
        match &lambda.parameters {
            LambdaParameters::Single(name) => vec![Some(name.value)],
            LambdaParameters::List(list) => list
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_ref().ok().map(|name| name.value))
                .collect(),
        }
    }

    pub(super) fn lambda_shape_matches(
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) -> bool {
        Self::lambda_parameter_names(lambda).len() == delegate.parameters.len()
    }

    /// Types a lambda body with known parameter types to learn its return type,
    /// then rolls every diagnostic back — this is inference, not checking.
    pub(super) fn probe_lambda_return(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) -> Option<Type> {
        let error_mark = self.resolver.out.errors.len();
        let saved_probe = self.lambda_probe_returns.take();
        let saved_return = std::mem::replace(&mut self.return_type, Type::Infer);
        let is_async = Self::is_async_lambda(lambda);
        let saved_async = std::mem::replace(&mut self.in_async, is_async);
        let iterator = self.enter_iterator_body(&Type::Void);

        let mut scope = Scope::default();
        for (name, parameter) in Self::lambda_parameter_names(lambda)
            .into_iter()
            .zip(&delegate.parameters)
        {
            if let Some(name) = name {
                scope
                    .locals
                    .insert(name, self.new_local(parameter.parameter_type.clone()));
            }
        }
        self.lambda_stack
            .push((EntityID::from(lambda), self.locals.len()));
        self.note_declared(&scope);
        self.locals.push(scope);

        let result = match &lambda.body {
            Ok(LambdaBody::Expression(expression)) => Some(self.check_expression(expression)),
            Ok(LambdaBody::Block(block)) => {
                self.lambda_probe_returns = Some(Vec::new());
                self.check_block(block);
                let returns = self.lambda_probe_returns.take().unwrap_or_default();
                if returns.is_empty() {
                    Some(Type::Void)
                } else {
                    let system = self.system();
                    best_common_type(&system, &returns)
                }
            }
            Err(()) => None,
        };

        self.locals.pop();
        self.lambda_stack.pop();
        self.leave_iterator_body(iterator, EntityID::from(lambda), lambda.span.clone());
        self.return_type = saved_return;
        self.in_async = saved_async;
        self.lambda_probe_returns = saved_probe;
        self.resolver.out.errors.truncate(error_mark);
        // an async lambda returns a task of what its body returns
        match result {
            Some(returned) if is_async => self.task_type(&returned),
            other => other,
        }
    }

    /// Checks a lambda against a concrete delegate signature, for real.
    pub(super) fn check_lambda_against(
        &mut self,
        lambda: &'ast LambdaExpression<'ast, 'ast>,
        delegate: &FunctionSignature,
    ) {
        let names = Self::lambda_parameter_names(lambda);
        if names.len() != delegate.parameters.len() {
            self.error(
                SemanticErrorKind::LambdaParameterMismatch,
                lambda.parameters.span(),
            );
            return;
        }
        // an async lambda's body returns what the delegate's task carries
        let is_async = Self::is_async_lambda(lambda);
        let unwrapped = if is_async {
            match self.async_signature(delegate, lambda.span.clone()) {
                Some(signature) => signature,
                None => return,
            }
        } else {
            delegate.clone()
        };
        let delegate = &unwrapped;

        let mut scope = Scope::default();
        for (name, parameter) in names.iter().zip(&delegate.parameters) {
            if let Some(name) = name {
                scope
                    .locals
                    .insert(*name, self.new_local(parameter.parameter_type.clone()));
            }
        }
        // explicitly written parameter types must agree with the delegate
        if let LambdaParameters::List(list) = &lambda.parameters {
            for (parameter, delegate_parameter) in list.parameters.iter().zip(&delegate.parameters)
            {
                if let Some(written) = &parameter.parameter_type {
                    let resolved = self.resolve_type(written);
                    if resolved != delegate_parameter.parameter_type
                        && !matches!(resolved, Type::Error)
                    {
                        let kind = SemanticErrorKind::TypeMismatch {
                            expected: self.describe(&delegate_parameter.parameter_type),
                            found: self.describe(&resolved),
                        };
                        self.error(kind, written.span.clone());
                    }
                }
            }
        }

        self.lambda_stack
            .push((EntityID::from(lambda), self.locals.len()));
        self.note_declared(&scope);
        self.locals.push(scope);
        let saved_return = std::mem::replace(&mut self.return_type, delegate.return_type.clone());
        let saved_async = std::mem::replace(&mut self.in_async, is_async);
        let iterator = self.enter_iterator_body(&Type::Void);

        match &lambda.body {
            Ok(LambdaBody::Expression(expression)) => {
                if delegate.return_type == Type::Void {
                    self.check_expression(expression);
                } else {
                    let expected = delegate.return_type.clone();
                    let literal = Self::is_integer_literal(expression);
                    let ty = self.check_expression_expecting(expression, Some(&expected));
                    self.require_convertible(&ty, &expected, literal, expression.span());
                }
            }
            Ok(LambdaBody::Block(block)) => self.check_block(block),
            Err(()) => {}
        }

        self.return_type = saved_return;
        self.in_async = saved_async;
        self.leave_iterator_body(iterator, EntityID::from(lambda), lambda.span.clone());
        self.locals.pop();
        self.lambda_stack.pop();
    }
}
