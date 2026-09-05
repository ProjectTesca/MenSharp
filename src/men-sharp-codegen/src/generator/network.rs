//! `[NetworkCallable]`: custom events with parameters over the network.
//!
//! Since SDK 3.7 a custom event may carry arguments: the sender calls
//! `SendCustomNetworkEvent(target, "Hit", 3, "me")`, the runtime serializes
//! the arguments by the *metadata* the receiving program declares for the
//! event — one entry per parameter, naming the heap variable to write and
//! its type — writes them into those variables on every receiver, and raises
//! the event. The variables are the ones a parameterized event already has
//! (see `programs`: `__0_damage__param`), so all a network-callable method
//! adds is the metadata, which the sidecar carries and the Unity side hands
//! to the SDK when the program asset is stored. Its event name is never
//! mangled: other scripts name it by its written name.
//!
//! The rules are UdonSharp's: public, an instance method, not virtual or an
//! override, not generic, not a built-in event, parameters of types the
//! engine can serialize.

use men_sharp_asm::NetworkCallable;
use men_sharp_parser::ast::{ArgumentValue, Expression, LiteralExpression, Modifier, PrimaryLeft};

use super::*;

impl<'a, 'ast> Generator<'a, 'ast> {
    /// Every `[NetworkCallable]` method of the entry chain: validated, and
    /// recorded on the program with the variables its arguments arrive in
    /// (its export layout's), for the sidecar.
    pub(super) fn record_network_callables(&mut self) {
        let classes = self.entry_chain.clone();
        for class in classes {
            let members: Vec<SymbolId> = self.declarations.table.symbol(class).members.to_vec();
            for member in members {
                if self.declarations.table.symbol(member).kind != SymbolKind::Method {
                    continue;
                }
                let Some(max_events_per_second) = self.network_callable_attribute(member) else {
                    continue;
                };
                let name = self.declarations.table.symbol(member).name.to_string();
                let (file, span) = self.declaration_site(member);
                let parameter_types: Vec<Type> = match self.signatures.members.get(&member) {
                    Some(MemberSignature::Function(signature)) => signature
                        .parameters
                        .iter()
                        .map(|parameter| parameter.parameter_type.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                let layout = self.export_layout(member);
                let recorded = match layout {
                    Some(layout) => self
                        .network_callable_refusal(member, &name, &layout, &parameter_types)
                        .map(|parameters| NetworkCallable {
                            event: layout.event,
                            max_events_per_second,
                            parameters,
                        }),
                    None => Err(format!("`{name}` cannot be a network callable event")),
                };
                match recorded {
                    Err(message) => self.errors.push(CodegenError {
                        message: message.into(),
                        file,
                        span,
                    }),
                    Ok(callable) => self.program.network_callables.push(callable),
                }
            }
        }
    }

    /// The parameter metadata of a network callable, or why it is refused.
    fn network_callable_refusal(
        &self,
        member: SymbolId,
        name: &str,
        layout: &programs::ExportLayout,
        parameter_types: &[Type],
    ) -> Result<Vec<(String, String)>, String> {
        let symbol = self.declarations.table.symbol(member);
        let refuse = |message: String| Err(message);
        if symbol.accessibility != Accessibility::Public {
            return refuse(format!(
                "a network callable method must be public: `{name}`"
            ));
        }
        if symbol.is_static {
            return refuse(format!(
                "a network callable method cannot be static: `{name}`"
            ));
        }
        if !symbol.type_parameters.is_empty() {
            return refuse(format!(
                "a network callable method cannot be generic: `{name}`"
            ));
        }
        let modifiers = self.declared_modifiers(member);
        if modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Virtual | Modifier::Abstract | Modifier::Override
            )
        }) {
            return refuse(format!(
                "a network callable method cannot be virtual, abstract or an override: `{name}`"
            ));
        }
        if self.nodes.event(name).is_some() {
            return refuse(format!(
                "`{name}` is a built-in event and cannot be marked [NetworkCallable]"
            ));
        }
        if parameter_types.len() > 8 {
            return refuse(format!(
                "a network callable method takes at most 8 parameters: `{name}` takes {}",
                parameter_types.len()
            ));
        }
        let mut parameters = Vec::new();
        for (variable, ty) in layout.parameters.iter().zip(parameter_types) {
            let Some(dotnet) = self.serializable_type_name(ty) else {
                return refuse(format!(
                    "a parameter of the network callable `{name}` has a type the network cannot \
                     carry; parameters must be engine or .NET types (numbers, strings, vectors, \
                     players, arrays of those) — not classes or structs of your own"
                ));
            };
            parameters.push((variable.clone(), dotnet));
        }
        Ok(parameters)
    }

    /// `[NetworkCallable]` / `[NetworkCallable(MaxEventsPerSecond = 5)]` on
    /// a member: `Some(rate)` when present (`None` inside for no rate).
    fn network_callable_attribute(&self, member: SymbolId) -> Option<Option<u32>> {
        for sections in self.attribute_sections(member) {
            for attribute in sections.iter().flat_map(|section| section.attributes) {
                let matches = attribute_name(attribute)
                    .map(|spelling| spelling.strip_suffix("Attribute").unwrap_or(spelling))
                    == Some("NetworkCallable");
                if !matches {
                    continue;
                }
                let mut rate = None;
                for argument in attribute
                    .arguments
                    .as_ref()
                    .map(|list| list.arguments)
                    .unwrap_or(&[])
                {
                    let named = argument.name.as_ref().is_some_and(|name| {
                        matches!(name.value, "MaxEventsPerSecond" | "maxEventsPerSecond")
                    });
                    // the one positional argument is the rate too
                    if named || argument.name.is_none() {
                        rate = integer_argument(&argument.value);
                    }
                }
                return Some(rate);
            }
        }
        None
    }

    /// The .NET full name of a type the network can serialize, `None` for
    /// one it cannot (a class or struct of the user's).
    fn serializable_type_name(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                arguments,
            } if arguments.is_empty() => Some(self.external.display_name(*id)),
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if self.declarations.table.symbol(*symbol).kind == SymbolKind::Enum => {
                // an enum of the user's is an Int32 on the heap
                Some("System.Int32".into())
            }
            Type::Array { element, rank: 1 } => {
                Some(format!("{}[]", self.serializable_type_name(element)?))
            }
            _ => None,
        }
    }

    /// A method declared on `MenSharpBehaviour` itself, called on *another*
    /// behaviour: `door.SendCustomEvent("Open")`,
    /// `door.SendCustomNetworkEvent(NetworkEventTarget.All, "Hit", 3)`,
    /// `door.RequestSerialization()`. On the behaviour itself these run
    /// through its own UdonBehaviour reference; on another one they are the
    /// same externs, on that program's receiver. `None` when the call is not
    /// that; an error when Udon offers no such extern across programs.
    pub(super) fn try_marker_member_call(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        receiver: DataId,
        values: &[DataId],
        span: Range<usize>,
    ) -> Option<Piece> {
        let marker = self.marker?;
        let entry = self.declarations.table.symbol(symbol);
        if entry.parent != Some(marker) || entry.kind != SymbolKind::Method {
            return None;
        }
        let name = entry.name.to_string();
        let signature = self.substitute_signature(&call.signature, &ctx.key.bindings);
        let mut parts = Vec::new();
        for parameter in &signature.parameters {
            parts.push(self.extern_type_name(&parameter.parameter_type)?);
        }
        let return_type = signature.return_type.clone();
        let result_part = if return_type == Type::Void {
            "SystemVoid".to_string()
        } else {
            self.extern_type_name(&return_type)?
        };
        let extern_signature = if parts.is_empty() {
            format!("{BEHAVIOUR_EXTERN_TYPE}.__{name}__{result_part}")
        } else {
            format!(
                "{BEHAVIOUR_EXTERN_TYPE}.__{name}__{}__{result_part}",
                parts.join("_")
            )
        };
        if !self.nodes.has_signature(&extern_signature) {
            self.error(
                ctx,
                Message::key("codegen.name_cannot_be_called_on_another_behaviour")
                    .arg("name", name),
                span,
            );
            return Some(Piece::Error);
        }
        let mut pushed = vec![receiver];
        pushed.extend(values.iter().copied());
        let result = (return_type != Type::Void).then(|| self.temp_for(&return_type));
        pushed.extend(result);
        self.call_extern(ctx, &extern_signature, &pushed, span);
        Some(match result {
            Some(result) => Piece::Value(result, return_type),
            None => Piece::Void,
        })
    }
}

/// An integer literal argument, as the attribute's rate.
fn integer_argument(value: &ArgumentValue<'_, '_>) -> Option<u32> {
    let ArgumentValue::Expression(Expression::Primary(primary)) = value else {
        return None;
    };
    let PrimaryLeft::Literal(LiteralExpression::Integer(text)) = &primary.left else {
        return None;
    };
    if !primary.chain.is_empty() {
        return None;
    }
    let raw: String = text.value.chars().filter(|c| *c != '_').collect();
    raw.trim_end_matches(['u', 'U', 'l', 'L']).parse().ok()
}
