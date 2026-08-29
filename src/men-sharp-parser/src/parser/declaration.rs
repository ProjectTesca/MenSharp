use std::ops::Range;

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        Accessor, AccessorKind, AccessorList, Attribute, AttributeSection, AttributeTarget,
        BaseTypeList, ClassDeclaration, ClassKind, ConstructorDeclaration, ConstructorInitializer,
        ConstructorInitializerKind, ConversionKind, DelegateDeclaration, DestructorDeclaration,
        Documents, EnumDeclaration, EnumMember, EventDeclaration, ExternAliasDirective,
        FieldDeclaration, FunctionBody, Ident, IndexerDeclaration, MethodDeclaration, Modifier,
        NameSegment, NameType, NamespaceDeclaration, NamespaceMember, OperatorDeclaration,
        OperatorSymbol, Parameter, ParameterList, PropertyDeclaration, Spanned, TypeDeclaration,
        TypeMember, UsingDirective, VariableDeclarator,
    },
    error::{ParseErrorKind, error_here, recover_until, recover_until_balanced},
    lexer::{Lexer, TokenKind},
    parser::{
        BumpVec, Errors, GreaterRun, ParserLexer, alloc_slice, consume_tokens,
        expression::{
            parse_argument_list, parse_expression, parse_expression_or_recover,
            parse_initializer_value, parse_parameter_modifiers,
        },
        parse_documents, peek_greater_run, skip_documents,
        statement::parse_block,
        types::{
            parse_generics_define, parse_generics_info, parse_type, parse_type_or_recover,
            parse_type_parameter_constraints,
        },
    },
};

/// Whether a declaration opens an `unsafe` scope, which is what makes `T*` a pointer type
/// rather than a multiplication.
pub(crate) fn has_unsafe(modifiers: &[Spanned<Modifier>]) -> bool {
    modifiers
        .iter()
        .any(|modifier| modifier.value == Modifier::Unsafe)
}

const MEMBER_RECOVERY: &[TokenKind] = &[
    TokenKind::Semicolon,
    TokenKind::BraceRight,
    TokenKind::BraceLeft,
];

// ============================================================================
// using directives and namespaces
// ============================================================================

/// `extern alias Foo;` -- always at the very top, before any `using`.
pub(crate) fn parse_extern_aliases<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [ExternAliasDirective<'input>] {
    let mut aliases = Vec::new_in(allocator);

    // `extern` is also a member modifier, so require the `alias` behind it
    while lexer.kind() == TokenKind::Extern && lexer.lookahead(1) == TokenKind::Alias {
        let anchor = lexer.cast_anchor();

        let extern_keyword = lexer.take_span();
        let alias_keyword = lexer.eat(TokenKind::Alias);

        let name = match lexer.eat_ident() {
            Some(name) => Ok(name),
            None => {
                errors.push(error_here(lexer, ParseErrorKind::MissingUsingTarget));
                Err(())
            }
        };

        let semicolon = match lexer.eat(TokenKind::Semicolon) {
            Some(semicolon) => Some(semicolon),
            None => {
                errors.push(error_here(lexer, ParseErrorKind::MissingSemicolonInUsing));
                None
            }
        };

        aliases.push(ExternAliasDirective {
            extern_keyword,
            alias_keyword,
            name,
            semicolon,
            span: anchor.elapsed(lexer),
        });
    }

    alloc_slice(allocator, aliases)
}

pub(crate) fn parse_using_directives<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [UsingDirective<'input, 'allocator>] {
    let mut directives = Vec::new_in(allocator);

    loop {
        // A doc comment here belongs to whatever declaration follows, so only step over it
        // once a `using` is confirmed behind it.
        let before_documents = lexer.cast_anchor();
        skip_documents(lexer);

        let is_using = lexer.kind() == TokenKind::Using
            || (lexer.kind() == TokenKind::Global && lexer.lookahead(1) == TokenKind::Using);
        if !is_using {
            lexer.back_to_anchor(before_documents);
            break;
        }

        let anchor = lexer.cast_anchor();

        let global = lexer.eat(TokenKind::Global);
        let using_keyword = lexer.take_span();
        let static_keyword = lexer.eat(TokenKind::Static);

        let alias = (lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::Equal)
            .then(|| {
                let alias = lexer.eat_ident().unwrap();
                lexer.next(); // `=`
                alias
            });

        let target = match parse_type(lexer, errors, allocator) {
            Some(target) => Ok(target),
            None => {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::Semicolon],
                    ParseErrorKind::MissingUsingTarget,
                ));
                Err(())
            }
        };

        let semicolon = match lexer.eat(TokenKind::Semicolon) {
            Some(semicolon) => Some(semicolon),
            None => {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::Semicolon, TokenKind::BraceLeft],
                    ParseErrorKind::MissingSemicolonInUsing,
                ));
                lexer.eat(TokenKind::Semicolon)
            }
        };

        directives.push(UsingDirective {
            global,
            using_keyword,
            static_keyword,
            alias,
            target,
            semicolon,
            span: anchor.elapsed(lexer),
        });
    }

    alloc_slice(allocator, directives)
}

pub(crate) fn parse_namespace_members<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    _until: &[TokenKind],
) -> &'allocator [NamespaceMember<'input, 'allocator>] {
    let mut members = Vec::new_in(allocator);

    loop {
        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        let anchor = lexer.cast_anchor();

        let documents = parse_documents(lexer, allocator);
        let attributes = parse_attribute_sections(lexer, errors, allocator);
        let modifiers = parse_modifiers(lexer, allocator);

        if lexer.kind() == TokenKind::Namespace {
            match parse_namespace_declaration(lexer, errors, allocator) {
                Some(namespace) => {
                    members.push(NamespaceMember::Namespace(namespace));
                    continue;
                }
                None => {
                    lexer.back_to_anchor(anchor);
                }
            }
        }

        match parse_type_declaration(lexer, errors, allocator, documents, attributes, modifiers) {
            Some(declaration) => members.push(NamespaceMember::Type(allocator.alloc(declaration))),
            None => {
                let before = lexer.cast_anchor();
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Semicolon, TokenKind::BraceRight],
                    ParseErrorKind::InvalidNamespaceMember,
                ));

                if before.elapsed(lexer).is_empty()
                    && lexer.eat(TokenKind::Semicolon).is_none()
                    && lexer.next().is_none()
                {
                    break;
                }
            }
        }
    }

    alloc_slice(allocator, members)
}

fn parse_namespace_declaration<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<NamespaceDeclaration<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let namespace_keyword = lexer.eat(TokenKind::Namespace)?;

    let mut segments = Vec::new_in(allocator);
    while let Some(segment) = lexer.eat_ident() {
        segments.push(segment);

        if lexer.eat(TokenKind::Dot).is_none() {
            break;
        }
    }

    let name = if segments.is_empty() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceLeft, TokenKind::Semicolon],
            ParseErrorKind::MissingNamespaceName,
        ));
        Err(())
    } else {
        Ok(alloc_slice(allocator, segments))
    };

    // `namespace A.B;` claims everything to the end of the file
    if lexer.eat(TokenKind::Semicolon).is_some() {
        let extern_aliases = parse_extern_aliases(lexer, errors, allocator);
        let usings = parse_using_directives(lexer, errors, allocator);
        let members = parse_namespace_members(lexer, errors, allocator, &[]);

        return Some(NamespaceDeclaration {
            namespace_keyword,
            extern_aliases,
            name,
            is_file_scoped: true,
            usings,
            members,
            span: anchor.elapsed(lexer),
        });
    }

    if lexer.eat(TokenKind::BraceLeft).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceLeft, TokenKind::BraceRight],
            ParseErrorKind::MissingNamespaceBody,
        ));

        return Some(NamespaceDeclaration {
            namespace_keyword,
            extern_aliases: &[],
            name,
            is_file_scoped: false,
            usings: &[],
            members: &[],
            span: anchor.elapsed(lexer),
        });
    }

    let extern_aliases = parse_extern_aliases(lexer, errors, allocator);
    let usings = parse_using_directives(lexer, errors, allocator);
    let members = parse_namespace_members(lexer, errors, allocator, &[]);

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedTypeBody,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    Some(NamespaceDeclaration {
        namespace_keyword,
        extern_aliases,
        name,
        is_file_scoped: false,
        usings,
        members,
        span: anchor.elapsed(lexer),
    })
}

// ============================================================================
// attributes and modifiers
// ============================================================================

/// Zero or more `[...]` sections.
///
/// Fully speculative: a `[` that does not turn out to hold attributes is put back
/// without an error, because the same token starts an element access and an array rank.
pub(crate) fn parse_attribute_sections<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [AttributeSection<'input, 'allocator>] {
    let mut sections = Vec::new_in(allocator);

    while lexer.kind() == TokenKind::BracketLeft {
        let anchor = lexer.cast_anchor();
        lexer.next();

        let target = parse_attribute_target(lexer);

        let mut attributes = Vec::new_in(allocator);
        loop {
            let attribute_anchor = lexer.cast_anchor();

            let Some(name) = parse_type(lexer, errors, allocator) else {
                break;
            };
            let arguments = (lexer.kind() == TokenKind::ParenthesisLeft)
                .then(|| parse_argument_list(lexer, errors, allocator, false));

            attributes.push(Attribute {
                name,
                arguments,
                span: attribute_anchor.elapsed(lexer),
            });

            if lexer.eat(TokenKind::Comma).is_none() {
                break;
            }
        }

        if attributes.is_empty() {
            lexer.back_to_anchor(anchor);
            break;
        }

        if lexer.eat(TokenKind::BracketRight).is_none() {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::BracketRight],
                ParseErrorKind::UnclosedAttributeSection,
            ));
            lexer.eat(TokenKind::BracketRight);
        }

        sections.push(AttributeSection {
            target,
            attributes: alloc_slice(allocator, attributes),
            span: anchor.elapsed(lexer),
        });
    }

    alloc_slice(allocator, sections)
}

fn parse_attribute_target(lexer: &mut Lexer<'_>) -> Option<Spanned<AttributeTarget>> {
    if lexer.lookahead(1) != TokenKind::Colon {
        return None;
    }

    let target = match lexer.kind() {
        TokenKind::Event => AttributeTarget::Event,
        TokenKind::Return => AttributeTarget::Return,
        TokenKind::Identifier => match lexer.current()?.text {
            "assembly" => AttributeTarget::Assembly,
            "module" => AttributeTarget::Module,
            "field" => AttributeTarget::Field,
            "method" => AttributeTarget::Method,
            "param" => AttributeTarget::Param,
            "property" => AttributeTarget::Property,
            "type" => AttributeTarget::Type,
            _ => return None,
        },
        _ => return None,
    };

    let span = lexer.take_span();
    lexer.next(); // `:`

    Some(Spanned::new(target, span))
}

pub(crate) fn parse_modifiers<'allocator>(
    lexer: &mut Lexer<'_>,
    allocator: &'allocator Bump,
) -> &'allocator [Spanned<Modifier>] {
    let mut modifiers: BumpVec<'allocator, Spanned<Modifier>> = Vec::new_in(allocator);

    loop {
        let modifier = match lexer.kind() {
            TokenKind::Public => Modifier::Public,
            TokenKind::Private => Modifier::Private,
            TokenKind::Protected => Modifier::Protected,
            TokenKind::Internal => Modifier::Internal,
            TokenKind::File => Modifier::File,
            TokenKind::Static => Modifier::Static,
            TokenKind::Readonly => Modifier::Readonly,
            TokenKind::Const => Modifier::Const,
            TokenKind::Volatile => Modifier::Volatile,
            TokenKind::Extern => Modifier::Extern,
            TokenKind::Unsafe => Modifier::Unsafe,
            TokenKind::Abstract => Modifier::Abstract,
            TokenKind::Virtual => Modifier::Virtual,
            TokenKind::Override => Modifier::Override,
            TokenKind::Sealed => Modifier::Sealed,
            TokenKind::New => Modifier::New,
            TokenKind::Async => Modifier::Async,
            TokenKind::Partial => Modifier::Partial,
            TokenKind::Required => Modifier::Required,
            TokenKind::Fixed => Modifier::Fixed,
            // `ref struct` -- a lone `ref` before a type is part of the type, not a modifier
            TokenKind::Ref if lexer.lookahead(1) == TokenKind::Struct => Modifier::Ref,
            _ => break,
        };

        modifiers.push(Spanned::new(modifier, lexer.take_span()));
    }

    alloc_slice(allocator, modifiers)
}

// ============================================================================
// type declarations
// ============================================================================

fn parse_type_declaration<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    documents: Documents<'input, 'allocator>,
    attributes: &'allocator [AttributeSection<'input, 'allocator>],
    modifiers: &'allocator [Spanned<Modifier>],
) -> Option<TypeDeclaration<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    match lexer.kind() {
        TokenKind::Enum => {
            let enum_keyword = lexer.take_span();

            let name = match lexer.eat_ident() {
                Some(name) => Ok(name),
                None => {
                    errors.push(recover_until(
                        lexer,
                        &[TokenKind::BraceLeft, TokenKind::Colon],
                        ParseErrorKind::MissingTypeName,
                    ));
                    Err(())
                }
            };

            let underlying_type = parse_base_type_list(lexer, errors, allocator);
            let members = parse_enum_members(lexer, errors, allocator);

            Some(TypeDeclaration::Enum(EnumDeclaration {
                documents,
                attributes,
                modifiers,
                enum_keyword,
                name,
                underlying_type,
                members,
                span: anchor.elapsed(lexer),
            }))
        }
        TokenKind::Delegate => {
            let delegate_keyword = lexer.take_span();

            let return_type = parse_type_or_recover(
                lexer,
                errors,
                allocator,
                &[TokenKind::Semicolon, TokenKind::ParenthesisLeft],
            );

            let name = match lexer.eat_ident() {
                Some(name) => Ok(name),
                None => {
                    errors.push(recover_until(
                        lexer,
                        &[TokenKind::Semicolon, TokenKind::ParenthesisLeft],
                        ParseErrorKind::MissingDelegateName,
                    ));
                    Err(())
                }
            };

            let generics = parse_generics_define(lexer, errors, allocator);
            let parameters = parse_parameter_list(lexer, errors, allocator);
            let constraints = parse_type_parameter_constraints(lexer, errors, allocator);

            let semicolon = match lexer.eat(TokenKind::Semicolon) {
                Some(semicolon) => Some(semicolon),
                None => {
                    errors.push(recover_until(
                        lexer,
                        &[TokenKind::Semicolon, TokenKind::BraceRight],
                        ParseErrorKind::MissingSemicolonInDelegate,
                    ));
                    lexer.eat(TokenKind::Semicolon)
                }
            };

            Some(TypeDeclaration::Delegate(DelegateDeclaration {
                documents,
                attributes,
                modifiers,
                delegate_keyword,
                return_type,
                name,
                generics,
                parameters,
                constraints,
                semicolon,
                span: anchor.elapsed(lexer),
            }))
        }
        TokenKind::Class | TokenKind::Struct | TokenKind::Interface | TokenKind::Record => {
            let keyword = lexer.kind();
            let start = lexer.take_span();

            let (class_kind, kind_span) = if keyword == TokenKind::Record {
                // `record`, `record class` and `record struct`
                match lexer.kind() {
                    TokenKind::Class => {
                        let end = lexer.take_span();
                        (ClassKind::Record, start.start..end.end)
                    }
                    TokenKind::Struct => {
                        let end = lexer.take_span();
                        (ClassKind::RecordStruct, start.start..end.end)
                    }
                    _ => (ClassKind::Record, start.clone()),
                }
            } else {
                let class_kind = match keyword {
                    TokenKind::Struct => ClassKind::Struct,
                    TokenKind::Interface => ClassKind::Interface,
                    _ => ClassKind::Class,
                };
                (class_kind, start.clone())
            };
            let kind = Spanned::new(class_kind, kind_span);

            let name = match lexer.eat_ident() {
                Some(name) => Ok(name),
                None => {
                    errors.push(recover_until(
                        lexer,
                        &[TokenKind::BraceLeft, TokenKind::Semicolon],
                        ParseErrorKind::MissingTypeName,
                    ));
                    Err(())
                }
            };

            let generics = parse_generics_define(lexer, errors, allocator);

            let primary_constructor = (lexer.kind() == TokenKind::ParenthesisLeft)
                .then(|| parse_parameter_list(lexer, errors, allocator).ok())
                .flatten();

            let base_types = parse_base_type_list(lexer, errors, allocator);
            let constraints = parse_type_parameter_constraints(lexer, errors, allocator);

            let is_unsafe = has_unsafe(modifiers);
            if is_unsafe {
                lexer.unsafe_depth += 1;
            }

            let members = if lexer.eat(TokenKind::Semicolon).is_some() {
                // `record Point(int X, int Y);` has no body at all
                Ok(&[] as &[TypeMember])
            } else if lexer.kind() == TokenKind::BraceLeft {
                Ok(parse_type_members(lexer, errors, allocator))
            } else {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::BraceLeft, TokenKind::BraceRight],
                    ParseErrorKind::MissingTypeBody,
                ));
                Err(())
            };

            if is_unsafe {
                lexer.unsafe_depth -= 1;
            }

            Some(TypeDeclaration::Class(ClassDeclaration {
                documents,
                attributes,
                modifiers,
                kind,
                name,
                generics,
                primary_constructor,
                base_types,
                constraints,
                members,
                span: anchor.elapsed(lexer),
            }))
        }
        _ => None,
    }
}

/// `: Base, IFoo` on a type, or `: byte` on an enum.
fn parse_base_type_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<BaseTypeList<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let colon = lexer.eat(TokenKind::Colon)?;

    let mut types = Vec::new_in(allocator);

    loop {
        match parse_type(lexer, errors, allocator) {
            Some(base_type) => types.push(base_type),
            None => {
                errors.push(recover_until(
                    lexer,
                    &[TokenKind::BraceLeft, TokenKind::Where, TokenKind::Semicolon],
                    ParseErrorKind::MissingBaseType,
                ));
                break;
            }
        }

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    Some(BaseTypeList {
        colon,
        types: alloc_slice(allocator, types),
        span: anchor.elapsed(lexer),
    })
}

#[allow(clippy::type_complexity)]
fn parse_enum_members<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<&'allocator [EnumMember<'input, 'allocator>], ()> {
    if lexer.eat(TokenKind::BraceLeft).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceLeft, TokenKind::BraceRight],
            ParseErrorKind::MissingTypeBody,
        ));
        return Err(());
    }

    let mut members = Vec::new_in(allocator);

    loop {
        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        let anchor = lexer.cast_anchor();

        let documents = parse_documents(lexer, allocator);
        let attributes = parse_attribute_sections(lexer, errors, allocator);

        let Some(name) = lexer.eat_ident() else {
            errors.push(recover_until_balanced(
                lexer,
                &[TokenKind::Comma, TokenKind::BraceRight],
                ParseErrorKind::MissingEnumMemberName,
            ));
            if lexer.eat(TokenKind::Comma).is_some() {
                continue;
            }
            break;
        };

        let value = lexer
            .eat(TokenKind::Equal)
            .and_then(|_| parse_expression(lexer, errors, allocator));

        members.push(EnumMember {
            documents,
            attributes,
            name,
            value,
            span: anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedTypeBody,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    Ok(alloc_slice(allocator, members))
}

// ============================================================================
// type members
// ============================================================================

fn parse_type_members<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> &'allocator [TypeMember<'input, 'allocator>] {
    lexer.eat(TokenKind::BraceLeft);

    let mut members = Vec::new_in(allocator);

    loop {
        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        let anchor = lexer.cast_anchor();

        let documents = parse_documents(lexer, allocator);
        let attributes = parse_attribute_sections(lexer, errors, allocator);
        let modifiers = parse_modifiers(lexer, allocator);

        let is_unsafe = has_unsafe(modifiers);
        if is_unsafe {
            lexer.unsafe_depth += 1;
        }
        let member = parse_type_member(
            lexer, errors, allocator, documents, attributes, modifiers, &anchor,
        );
        if is_unsafe {
            lexer.unsafe_depth -= 1;
        }

        match member {
            Some(member) => members.push(member),
            None => {
                let before = lexer.cast_anchor();
                errors.push(recover_until_balanced(
                    lexer,
                    MEMBER_RECOVERY,
                    ParseErrorKind::InvalidTypeMember,
                ));

                if before.elapsed(lexer).is_empty()
                    && lexer.eat(TokenKind::Semicolon).is_none()
                    && lexer.next().is_none()
                {
                    break;
                }
            }
        }
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedTypeBody,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    alloc_slice(allocator, members)
}

#[allow(clippy::too_many_arguments)]
fn parse_type_member<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    documents: Documents<'input, 'allocator>,
    attributes: &'allocator [AttributeSection<'input, 'allocator>],
    modifiers: &'allocator [Spanned<Modifier>],
    anchor: &crate::lexer::Anchor,
) -> Option<TypeMember<'input, 'allocator>> {
    // nested types
    if matches!(
        lexer.kind(),
        TokenKind::Class
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::Record
            | TokenKind::Enum
            | TokenKind::Delegate
    ) {
        return parse_type_declaration(lexer, errors, allocator, documents, attributes, modifiers)
            .map(TypeMember::NestedType);
    }

    // `~Foo() { }`
    if let Some(tilde) = lexer.eat(TokenKind::Tilde) {
        let name = match lexer.eat_ident() {
            Some(name) => Ok(name),
            None => {
                errors.push(recover_until(
                    lexer,
                    MEMBER_RECOVERY,
                    ParseErrorKind::MissingMemberName,
                ));
                Err(())
            }
        };
        let parameters = parse_parameter_list(lexer, errors, allocator);
        let body = parse_function_body(lexer, errors, allocator);

        return Some(TypeMember::Destructor(DestructorDeclaration {
            documents,
            attributes,
            modifiers,
            tilde,
            name,
            parameters,
            body,
            span: anchor.elapsed(lexer),
        }));
    }

    // `event Action Foo;` / `event Action Foo { add { } remove { } }`
    if let Some(event_keyword) = lexer.eat(TokenKind::Event) {
        let event_type = parse_type_or_recover(lexer, errors, allocator, MEMBER_RECOVERY).ok()?;
        let (explicit_interface, name) = match parse_member_name(lexer, errors, allocator)? {
            MemberName::Named {
                explicit_interface,
                name,
            } => (explicit_interface, name),
            MemberName::Indexer { .. } => return None,
        };

        let mut declarators = Vec::new_in(allocator);
        let mut accessors = None;

        if lexer.kind() == TokenKind::BraceLeft {
            declarators.push(VariableDeclarator {
                name: name.clone(),
                initializer: None,
                span: name.span.clone(),
            });
            accessors = Some(parse_accessor_list(lexer, errors, allocator));
        } else {
            let mut current = name;
            loop {
                let initializer = lexer
                    .eat(TokenKind::Equal)
                    .and_then(|_| parse_initializer_value(lexer, errors, allocator));
                let span = current.span.clone();

                declarators.push(VariableDeclarator {
                    name: current,
                    initializer,
                    span,
                });

                if lexer.eat(TokenKind::Comma).is_none() {
                    break;
                }
                match lexer.eat_ident() {
                    Some(next) => current = next,
                    None => break,
                }
            }
        }

        let semicolon = (accessors.is_none())
            .then(|| eat_member_semicolon(lexer, errors))
            .flatten();

        return Some(TypeMember::Event(EventDeclaration {
            documents,
            attributes,
            modifiers,
            event_keyword,
            event_type,
            explicit_interface,
            declarators: alloc_slice(allocator, declarators),
            accessors,
            semicolon,
            span: anchor.elapsed(lexer),
        }));
    }

    // `implicit operator T(...)` / `explicit operator T(...)`
    let conversion = match lexer.kind() {
        TokenKind::Implicit => Some(Spanned::new(ConversionKind::Implicit, lexer.take_span())),
        TokenKind::Explicit => Some(Spanned::new(ConversionKind::Explicit, lexer.take_span())),
        _ => None,
    };
    if conversion.is_some() {
        let operator_keyword = match lexer.eat(TokenKind::Operator) {
            Some(operator_keyword) => operator_keyword,
            None => {
                errors.push(recover_until(
                    lexer,
                    MEMBER_RECOVERY,
                    ParseErrorKind::MissingOperatorSymbol,
                ));
                return None;
            }
        };
        let result_type = parse_type_or_recover(lexer, errors, allocator, MEMBER_RECOVERY).ok()?;
        let parameters = parse_parameter_list(lexer, errors, allocator);
        let body = parse_function_body(lexer, errors, allocator);

        return Some(TypeMember::Operator(OperatorDeclaration {
            documents,
            attributes,
            modifiers,
            conversion,
            operator_keyword,
            result_type,
            symbol: None,
            parameters,
            body,
            span: anchor.elapsed(lexer),
        }));
    }

    // `Foo(...)` with no return type in front is a constructor
    if lexer.kind().can_be_identifier() && lexer.lookahead(1) == TokenKind::ParenthesisLeft {
        let name = lexer.eat_ident().unwrap();
        let parameters = parse_parameter_list(lexer, errors, allocator);

        let initializer = lexer.eat(TokenKind::Colon).map(|colon| {
            let initializer_anchor = lexer.cast_anchor();

            let kind = match lexer.kind() {
                TokenKind::Base => {
                    Spanned::new(ConstructorInitializerKind::Base, lexer.take_span())
                }
                _ => Spanned::new(ConstructorInitializerKind::This, lexer.take_span()),
            };

            let arguments = if lexer.kind() == TokenKind::ParenthesisLeft {
                Ok(parse_argument_list(lexer, errors, allocator, false))
            } else {
                errors.push(recover_until(
                    lexer,
                    MEMBER_RECOVERY,
                    ParseErrorKind::MissingConstructorInitializerArguments,
                ));
                Err(())
            };

            ConstructorInitializer {
                colon,
                kind,
                arguments,
                span: initializer_anchor.elapsed(lexer),
            }
        });

        let body = parse_function_body(lexer, errors, allocator);

        return Some(TypeMember::Constructor(ConstructorDeclaration {
            documents,
            attributes,
            modifiers,
            name,
            parameters,
            initializer,
            body,
            span: anchor.elapsed(lexer),
        }));
    }

    // everything else starts with a type
    let member_type = parse_type(lexer, errors, allocator)?;

    // `Foo operator +(...)`
    if let Some(operator_keyword) = lexer.eat(TokenKind::Operator) {
        let symbol = parse_operator_symbol(lexer, errors);
        let parameters = parse_parameter_list(lexer, errors, allocator);
        let body = parse_function_body(lexer, errors, allocator);

        return Some(TypeMember::Operator(OperatorDeclaration {
            documents,
            attributes,
            modifiers,
            conversion: None,
            operator_keyword,
            result_type: member_type,
            symbol,
            parameters,
            body,
            span: anchor.elapsed(lexer),
        }));
    }

    match parse_member_name(lexer, errors, allocator)? {
        MemberName::Indexer {
            explicit_interface,
            this_keyword,
        } => {
            let parameters = if lexer.kind() == TokenKind::BracketLeft {
                parse_bracketed_parameter_list(lexer, errors, allocator)
            } else {
                errors.push(recover_until(
                    lexer,
                    MEMBER_RECOVERY,
                    ParseErrorKind::MissingIndexerParameters,
                ));
                Err(())
            };

            // like a property, an indexer's `{ ... }` holds accessors rather than statements
            let body = if lexer.kind() == TokenKind::BraceLeft {
                FunctionBody::Accessors(parse_accessor_list(lexer, errors, allocator))
            } else {
                parse_function_body(lexer, errors, allocator)
            };

            Some(TypeMember::Indexer(IndexerDeclaration {
                documents,
                attributes,
                modifiers,
                element_type: member_type,
                explicit_interface,
                this_keyword,
                parameters,
                body,
                span: anchor.elapsed(lexer),
            }))
        }
        MemberName::Named {
            explicit_interface,
            name,
        } => {
            let generics = parse_generics_define(lexer, errors, allocator);

            if lexer.kind() == TokenKind::ParenthesisLeft {
                let parameters = parse_parameter_list(lexer, errors, allocator);
                let constraints = parse_type_parameter_constraints(lexer, errors, allocator);
                let body = parse_function_body(lexer, errors, allocator);

                return Some(TypeMember::Method(MethodDeclaration {
                    documents,
                    attributes,
                    modifiers,
                    return_type: member_type,
                    explicit_interface,
                    name,
                    generics,
                    parameters,
                    constraints,
                    body,
                    span: anchor.elapsed(lexer),
                }));
            }

            if matches!(lexer.kind(), TokenKind::BraceLeft | TokenKind::FatArrow) {
                // a `{` after a property name holds accessors, not statements
                let (body, initializer) = if lexer.kind() == TokenKind::BraceLeft {
                    let accessors = parse_accessor_list(lexer, errors, allocator);

                    // `public int X { get; set; } = 1;`
                    let initializer = lexer
                        .eat(TokenKind::Equal)
                        .and_then(|_| parse_initializer_value(lexer, errors, allocator));
                    if initializer.is_some() {
                        eat_member_semicolon(lexer, errors);
                    }

                    (FunctionBody::Accessors(accessors), initializer)
                } else {
                    (parse_function_body(lexer, errors, allocator), None)
                };

                return Some(TypeMember::Property(PropertyDeclaration {
                    documents,
                    attributes,
                    modifiers,
                    property_type: member_type,
                    explicit_interface,
                    name,
                    body,
                    initializer,
                    span: anchor.elapsed(lexer),
                }));
            }

            // a field
            let mut declarators = Vec::new_in(allocator);
            let mut current = name;

            loop {
                let initializer = lexer
                    .eat(TokenKind::Equal)
                    .and_then(|_| parse_initializer_value(lexer, errors, allocator));
                let start = current.span.start;
                let end = initializer
                    .as_ref()
                    .map(|initializer| initializer.span().end)
                    .unwrap_or(current.span.end);

                declarators.push(VariableDeclarator {
                    name: current,
                    initializer,
                    span: start..end,
                });

                if lexer.eat(TokenKind::Comma).is_none() {
                    break;
                }
                match lexer.eat_ident() {
                    Some(next) => current = next,
                    None => {
                        errors.push(recover_until(
                            lexer,
                            MEMBER_RECOVERY,
                            ParseErrorKind::MissingMemberName,
                        ));
                        break;
                    }
                }
            }

            let semicolon = eat_member_semicolon(lexer, errors);

            Some(TypeMember::Field(FieldDeclaration {
                documents,
                attributes,
                modifiers,
                field_type: member_type,
                declarators: alloc_slice(allocator, declarators),
                semicolon,
                span: anchor.elapsed(lexer),
            }))
        }
    }
}

/// As with statements, a missing `;` is reported where it belongs instead of skipped to,
/// so the member that follows still gets parsed.
fn eat_member_semicolon(lexer: &mut Lexer<'_>, errors: &mut Errors) -> Option<Range<usize>> {
    match lexer.eat(TokenKind::Semicolon) {
        Some(semicolon) => Some(semicolon),
        None => {
            errors.push(error_here(lexer, ParseErrorKind::MissingSemicolonInMember));
            None
        }
    }
}

/// The name of a member, which may be qualified by the interface it explicitly implements.
enum MemberName<'input, 'allocator> {
    Named {
        explicit_interface: Option<NameType<'input, 'allocator>>,
        name: Ident<'input>,
    },
    Indexer {
        explicit_interface: Option<NameType<'input, 'allocator>>,
        this_keyword: Range<usize>,
    },
}

/// Reads `Name`, `IFoo.Name`, `this` or `IFoo.this`.
///
/// Every segment before the last belongs to the interface; the last one is the member.
/// A generic argument list on the final segment is left unconsumed, because a method
/// declaration wants type *parameters* there, not arguments.
fn parse_member_name<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Option<MemberName<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let mut segments: BumpVec<'allocator, NameSegment<'input, 'allocator>> = Vec::new_in(allocator);

    loop {
        if let Some(this_keyword) = lexer.eat(TokenKind::This) {
            return Some(MemberName::Indexer {
                explicit_interface: into_explicit_interface(segments, allocator),
                this_keyword,
            });
        }

        let segment_anchor = lexer.cast_anchor();
        let Some(name) = lexer.eat_ident() else {
            lexer.back_to_anchor(anchor);
            return None;
        };

        let generics_anchor = lexer.cast_anchor();
        let generics = parse_generics_info(lexer, errors, allocator);

        if lexer.kind() != TokenKind::Dot {
            // the final segment: hand its type parameter list back to the caller
            lexer.back_to_anchor(generics_anchor);

            return Some(MemberName::Named {
                explicit_interface: into_explicit_interface(segments, allocator),
                name,
            });
        }
        lexer.next(); // `.`

        segments.push(NameSegment {
            name,
            generics,
            span: segment_anchor.elapsed(lexer),
        });
    }
}

fn into_explicit_interface<'input, 'allocator>(
    segments: BumpVec<'allocator, NameSegment<'input, 'allocator>>,
    allocator: &'allocator Bump,
) -> Option<NameType<'input, 'allocator>> {
    let start = segments.first()?.span.start;
    let end = segments.last()?.span.end;

    Some(NameType {
        global: None,
        segments: alloc_slice(allocator, segments),
        span: start..end,
    })
}

fn parse_operator_symbol(
    lexer: &mut Lexer<'_>,
    errors: &mut Errors,
) -> Option<Spanned<OperatorSymbol>> {
    let symbol = match lexer.kind() {
        TokenKind::Plus => OperatorSymbol::Plus,
        TokenKind::Minus => OperatorSymbol::Minus,
        TokenKind::Exclamation => OperatorSymbol::Not,
        TokenKind::Tilde => OperatorSymbol::BitwiseNot,
        TokenKind::DoublePlus => OperatorSymbol::Increment,
        TokenKind::DoubleMinus => OperatorSymbol::Decrement,
        TokenKind::True => OperatorSymbol::True,
        TokenKind::False => OperatorSymbol::False,
        TokenKind::Asterisk => OperatorSymbol::Multiply,
        TokenKind::Slash => OperatorSymbol::Divide,
        TokenKind::Percent => OperatorSymbol::Modulo,
        TokenKind::Ampersand => OperatorSymbol::BitwiseAnd,
        TokenKind::VerticalLine => OperatorSymbol::BitwiseOr,
        TokenKind::Circumflex => OperatorSymbol::BitwiseXor,
        TokenKind::LeftShift => OperatorSymbol::LeftShift,
        TokenKind::DoubleEqual => OperatorSymbol::Equal,
        TokenKind::ExclamationEqual => OperatorSymbol::NotEqual,
        TokenKind::LessThan => OperatorSymbol::LessThan,
        TokenKind::LessThanEqual => OperatorSymbol::LessThanEqual,
        TokenKind::GreaterThan | TokenKind::GreaterThanEqual => {
            let (run, span, count) = peek_greater_run(lexer)?;
            let symbol = match run {
                GreaterRun::Greater => OperatorSymbol::GreaterThan,
                GreaterRun::GreaterEqual => OperatorSymbol::GreaterThanEqual,
                GreaterRun::RightShift => OperatorSymbol::RightShift,
                GreaterRun::UnsignedRightShift => OperatorSymbol::UnsignedRightShift,
                _ => {
                    errors.push(recover_until(
                        lexer,
                        MEMBER_RECOVERY,
                        ParseErrorKind::MissingOperatorSymbol,
                    ));
                    return None;
                }
            };
            consume_tokens(lexer, count);
            return Some(Spanned::new(symbol, span));
        }
        _ => {
            errors.push(recover_until(
                lexer,
                MEMBER_RECOVERY,
                ParseErrorKind::MissingOperatorSymbol,
            ));
            return None;
        }
    };

    Some(Spanned::new(symbol, lexer.take_span()))
}

// ============================================================================
// accessors, parameters and bodies
// ============================================================================

fn parse_accessor_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> AccessorList<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    lexer.eat(TokenKind::BraceLeft);

    let mut accessors = Vec::new_in(allocator);

    loop {
        if matches!(lexer.kind(), TokenKind::BraceRight | TokenKind::None) {
            break;
        }

        let accessor_anchor = lexer.cast_anchor();

        let attributes = parse_attribute_sections(lexer, errors, allocator);
        let modifiers = parse_modifiers(lexer, allocator);

        let kind = match lexer.kind() {
            TokenKind::Get => AccessorKind::Get,
            TokenKind::Set => AccessorKind::Set,
            TokenKind::Init => AccessorKind::Init,
            TokenKind::Add => AccessorKind::Add,
            TokenKind::Remove => AccessorKind::Remove,
            _ => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::BraceRight],
                    ParseErrorKind::InvalidAccessor,
                ));
                break;
            }
        };
        let kind = Spanned::new(kind, lexer.take_span());

        let body = parse_function_body(lexer, errors, allocator);

        accessors.push(Accessor {
            attributes,
            modifiers,
            kind,
            body,
            span: accessor_anchor.elapsed(lexer),
        });
    }

    if lexer.eat(TokenKind::BraceRight).is_none() {
        errors.push(recover_until(
            lexer,
            &[TokenKind::BraceRight],
            ParseErrorKind::UnclosedAccessorList,
        ));
        lexer.eat(TokenKind::BraceRight);
    }

    AccessorList {
        accessors: alloc_slice(allocator, accessors),
        span: anchor.elapsed(lexer),
    }
}

pub(crate) fn parse_function_body<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> FunctionBody<'input, 'allocator> {
    if let Some(semicolon) = lexer.eat(TokenKind::Semicolon) {
        return FunctionBody::None { semicolon };
    }

    if let Some(fat_arrow) = lexer.eat(TokenKind::FatArrow) {
        let anchor_start = fat_arrow.start;
        let expression = parse_expression_or_recover(lexer, errors, allocator, MEMBER_RECOVERY);
        let semicolon = eat_member_semicolon(lexer, errors);
        let end = semicolon
            .as_ref()
            .map(|semicolon| semicolon.end)
            .unwrap_or(fat_arrow.end);

        return FunctionBody::Expression {
            fat_arrow,
            expression,
            semicolon,
            span: anchor_start..end,
        };
    }

    match parse_block(lexer, errors, allocator) {
        Some(block) => FunctionBody::Block(block),
        None => {
            errors.push(recover_until_balanced(
                lexer,
                MEMBER_RECOVERY,
                ParseErrorKind::MissingMemberBody,
            ));
            FunctionBody::Missing
        }
    }
}

pub(crate) fn parse_parameter_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<ParameterList<'input, 'allocator>, ()> {
    parse_delimited_parameter_list(lexer, errors, allocator, false)
}

fn parse_bracketed_parameter_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
) -> Result<ParameterList<'input, 'allocator>, ()> {
    parse_delimited_parameter_list(lexer, errors, allocator, true)
}

fn parse_delimited_parameter_list<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut Errors,
    allocator: &'allocator Bump,
    bracket: bool,
) -> Result<ParameterList<'input, 'allocator>, ()> {
    let (open, close) = if bracket {
        (TokenKind::BracketLeft, TokenKind::BracketRight)
    } else {
        (TokenKind::ParenthesisLeft, TokenKind::ParenthesisRight)
    };

    let anchor = lexer.cast_anchor();

    if lexer.eat(open).is_none() {
        errors.push(recover_until(
            lexer,
            MEMBER_RECOVERY,
            ParseErrorKind::MissingParameterList,
        ));
        return Err(());
    }

    let mut parameters = Vec::new_in(allocator);

    loop {
        if lexer.kind() == close || lexer.kind() == TokenKind::None {
            break;
        }

        let parameter_anchor = lexer.cast_anchor();

        let attributes = parse_attribute_sections(lexer, errors, allocator);
        let modifiers = parse_parameter_modifiers(lexer, allocator);

        let parameter_type = parse_type(lexer, errors, allocator);

        let name = match lexer.eat_ident() {
            Some(name) => Ok(name),
            None => {
                errors.push(recover_until_balanced(
                    lexer,
                    &[TokenKind::Comma, close],
                    ParseErrorKind::MissingParameterName,
                ));
                Err(())
            }
        };

        let default_value = lexer.eat(TokenKind::Equal).and_then(|_| {
            match parse_expression(lexer, errors, allocator) {
                Some(default_value) => Some(default_value),
                None => {
                    errors.push(recover_until_balanced(
                        lexer,
                        &[TokenKind::Comma, close],
                        ParseErrorKind::MissingDefaultValue,
                    ));
                    None
                }
            }
        });

        parameters.push(Parameter {
            attributes,
            modifiers,
            parameter_type,
            name,
            default_value,
            span: parameter_anchor.elapsed(lexer),
        });

        if lexer.eat(TokenKind::Comma).is_none() {
            break;
        }
    }

    if lexer.eat(close).is_none() {
        errors.push(recover_until_balanced(
            lexer,
            &[close, TokenKind::BraceLeft, TokenKind::Semicolon],
            ParseErrorKind::UnclosedParameterList,
        ));
        lexer.eat(close);
    }

    Ok(ParameterList {
        parameters: alloc_slice(allocator, parameters),
        span: anchor.elapsed(lexer),
    })
}
