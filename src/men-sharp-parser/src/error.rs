use std::ops::Range;

use men_sharp_diagnostics::{Edit, Hint, Message};

use crate::lexer::{GetKind, Lexer, TokenKind};

/// A parse error is deliberately just a kind plus the source range it covers.
/// Rendering (line/column, snippets, labels) is the diagnostic layer's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    // ---------- compilation unit / namespace ----------
    InvalidNamespaceMember,
    MissingNamespaceName,
    MissingNamespaceBody,
    MissingUsingTarget,
    MissingSemicolonInUsing,
    // ---------- type declarations ----------
    MissingTypeName,
    MissingTypeBody,
    UnclosedTypeBody,
    InvalidTypeMember,
    MissingBaseType,
    /// `record Derived : Base(1)` with no primary constructor of its own.
    BaseArgumentsWithoutPrimaryConstructor,
    MissingEnumMemberName,
    MissingDelegateName,
    MissingSemicolonInDelegate,
    // ---------- generics ----------
    MissingGenericsParameterName,
    UnclosedGenericsDefine,
    UnclosedGenericsInfo,
    MissingConstraintTarget,
    MissingConstraintBound,
    // ---------- members ----------
    MissingMemberName,
    MissingMemberBody,
    MissingParameterList,
    UnclosedParameterList,
    MissingParameterName,
    MissingDefaultValue,
    MissingAccessorList,
    UnclosedAccessorList,
    InvalidAccessor,
    MissingSemicolonInMember,
    MissingConstructorInitializerArguments,
    MissingOperatorSymbol,
    MissingIndexerParameters,
    // ---------- attributes ----------
    UnclosedAttributeSection,
    MissingAttributeName,
    // ---------- types ----------
    MissingType,
    UnclosedArrayRank,
    UnclosedTupleType,
    MissingTupleTypeElement,
    // ---------- statements ----------
    InvalidStatement,
    UnclosedBlock,
    MissingSemicolon,
    MissingCondition,
    MissingParenthesisLeft,
    UnclosedParenthesis,
    MissingStatement,
    MissingWhileInDoWhile,
    MissingInInForeach,
    MissingForeachVariable,
    MissingCatchOrFinally,
    UnclosedSwitchBody,
    MissingCaseLabel,
    MissingColonInSwitchLabel,
    MissingLabelName,
    MissingUsingResource,
    // ---------- expressions ----------
    MissingExpression,
    MissingRightOperand,
    MissingColonInConditional,
    MissingBranchInConditional,
    UnclosedArgumentList,
    UnclosedElementAccess,
    MissingMemberNameAfterSeparator,
    MissingLambdaBody,
    UnclosedInitializer,
    MissingInitializerValue,
    MissingArraySize,
    MissingTypeInNewExpression,
    UnclosedSwitchExpression,
    MissingSwitchExpressionArm,
    MissingFatArrowInSwitchArm,
    // ---------- interpolated strings ----------
    MissingInterpolationExpression,
    InvalidInterpolationHole,
    // ---------- query expressions ----------
    MissingQueryClause,
    MissingSelectOrGroup,
    MissingQueryContinuation,
    // ---------- patterns ----------
    MissingPattern,
    UnclosedPropertyPattern,
    MissingSubpatternName,
    UnclosedParenthesizedPattern,
    UnclosedListPattern,
}

/// Consumes tokens until one of `until` (or the end of input) is reached, and reports
/// everything skipped as a single error.
///
/// This is the only recovery primitive the parser uses: a caller that cannot make progress
/// picks the token kinds that reliably re-synchronise its own grammar rule (usually
/// `;`, `}` or `,`) and hands the rest of the decision to the enclosing rule.
/// Where a hint's token goes, relative to the error's span.
enum Placement {
    /// At the start: the token that was expected where the error begins.
    Start,
    /// At the end: what would close the construct the error spans.
    End,
}

impl ParseErrorKind {
    /// The message catalog key.
    pub fn message_key(self) -> &'static str {
        match self {
            ParseErrorKind::InvalidNamespaceMember => "syntax.invalid_namespace_member",
            ParseErrorKind::MissingNamespaceName => "syntax.missing_namespace_name",
            ParseErrorKind::MissingNamespaceBody => "syntax.missing_namespace_body",
            ParseErrorKind::MissingUsingTarget => "syntax.missing_using_target",
            ParseErrorKind::MissingSemicolonInUsing => "syntax.missing_semicolon_in_using",
            ParseErrorKind::MissingTypeName => "syntax.missing_type_name",
            ParseErrorKind::MissingTypeBody => "syntax.missing_type_body",
            ParseErrorKind::UnclosedTypeBody => "syntax.unclosed_type_body",
            ParseErrorKind::InvalidTypeMember => "syntax.invalid_type_member",
            ParseErrorKind::MissingBaseType => "syntax.missing_base_type",
            ParseErrorKind::BaseArgumentsWithoutPrimaryConstructor => {
                "syntax.base_arguments_without_primary_constructor"
            }
            ParseErrorKind::MissingEnumMemberName => "syntax.missing_enum_member_name",
            ParseErrorKind::MissingDelegateName => "syntax.missing_delegate_name",
            ParseErrorKind::MissingSemicolonInDelegate => "syntax.missing_semicolon_in_delegate",
            ParseErrorKind::MissingGenericsParameterName => {
                "syntax.missing_generics_parameter_name"
            }
            ParseErrorKind::UnclosedGenericsDefine => "syntax.unclosed_generics_define",
            ParseErrorKind::UnclosedGenericsInfo => "syntax.unclosed_generics_info",
            ParseErrorKind::MissingConstraintTarget => "syntax.missing_constraint_target",
            ParseErrorKind::MissingConstraintBound => "syntax.missing_constraint_bound",
            ParseErrorKind::MissingMemberName => "syntax.missing_member_name",
            ParseErrorKind::MissingMemberBody => "syntax.missing_member_body",
            ParseErrorKind::MissingParameterList => "syntax.missing_parameter_list",
            ParseErrorKind::UnclosedParameterList => "syntax.unclosed_parameter_list",
            ParseErrorKind::MissingParameterName => "syntax.missing_parameter_name",
            ParseErrorKind::MissingDefaultValue => "syntax.missing_default_value",
            ParseErrorKind::MissingAccessorList => "syntax.missing_accessor_list",
            ParseErrorKind::UnclosedAccessorList => "syntax.unclosed_accessor_list",
            ParseErrorKind::InvalidAccessor => "syntax.invalid_accessor",
            ParseErrorKind::MissingSemicolonInMember => "syntax.missing_semicolon_in_member",
            ParseErrorKind::MissingConstructorInitializerArguments => {
                "syntax.missing_constructor_initializer_arguments"
            }
            ParseErrorKind::MissingOperatorSymbol => "syntax.missing_operator_symbol",
            ParseErrorKind::MissingIndexerParameters => "syntax.missing_indexer_parameters",
            ParseErrorKind::UnclosedAttributeSection => "syntax.unclosed_attribute_section",
            ParseErrorKind::MissingAttributeName => "syntax.missing_attribute_name",
            ParseErrorKind::MissingType => "syntax.missing_type",
            ParseErrorKind::UnclosedArrayRank => "syntax.unclosed_array_rank",
            ParseErrorKind::UnclosedTupleType => "syntax.unclosed_tuple_type",
            ParseErrorKind::MissingTupleTypeElement => "syntax.missing_tuple_type_element",
            ParseErrorKind::InvalidStatement => "syntax.invalid_statement",
            ParseErrorKind::UnclosedBlock => "syntax.unclosed_block",
            ParseErrorKind::MissingSemicolon => "syntax.missing_semicolon",
            ParseErrorKind::MissingCondition => "syntax.missing_condition",
            ParseErrorKind::MissingParenthesisLeft => "syntax.missing_parenthesis_left",
            ParseErrorKind::UnclosedParenthesis => "syntax.unclosed_parenthesis",
            ParseErrorKind::MissingStatement => "syntax.missing_statement",
            ParseErrorKind::MissingWhileInDoWhile => "syntax.missing_while_in_do_while",
            ParseErrorKind::MissingInInForeach => "syntax.missing_in_in_foreach",
            ParseErrorKind::MissingForeachVariable => "syntax.missing_foreach_variable",
            ParseErrorKind::MissingCatchOrFinally => "syntax.missing_catch_or_finally",
            ParseErrorKind::UnclosedSwitchBody => "syntax.unclosed_switch_body",
            ParseErrorKind::MissingCaseLabel => "syntax.missing_case_label",
            ParseErrorKind::MissingColonInSwitchLabel => "syntax.missing_colon_in_switch_label",
            ParseErrorKind::MissingLabelName => "syntax.missing_label_name",
            ParseErrorKind::MissingUsingResource => "syntax.missing_using_resource",
            ParseErrorKind::MissingExpression => "syntax.missing_expression",
            ParseErrorKind::MissingRightOperand => "syntax.missing_right_operand",
            ParseErrorKind::MissingColonInConditional => "syntax.missing_colon_in_conditional",
            ParseErrorKind::MissingBranchInConditional => "syntax.missing_branch_in_conditional",
            ParseErrorKind::UnclosedArgumentList => "syntax.unclosed_argument_list",
            ParseErrorKind::UnclosedElementAccess => "syntax.unclosed_element_access",
            ParseErrorKind::MissingMemberNameAfterSeparator => {
                "syntax.missing_member_name_after_separator"
            }
            ParseErrorKind::MissingLambdaBody => "syntax.missing_lambda_body",
            ParseErrorKind::UnclosedInitializer => "syntax.unclosed_initializer",
            ParseErrorKind::MissingInitializerValue => "syntax.missing_initializer_value",
            ParseErrorKind::MissingArraySize => "syntax.missing_array_size",
            ParseErrorKind::MissingTypeInNewExpression => "syntax.missing_type_in_new_expression",
            ParseErrorKind::UnclosedSwitchExpression => "syntax.unclosed_switch_expression",
            ParseErrorKind::MissingSwitchExpressionArm => "syntax.missing_switch_expression_arm",
            ParseErrorKind::MissingFatArrowInSwitchArm => "syntax.missing_fat_arrow_in_switch_arm",
            ParseErrorKind::MissingInterpolationExpression => {
                "syntax.missing_interpolation_expression"
            }
            ParseErrorKind::InvalidInterpolationHole => "syntax.invalid_interpolation_hole",
            ParseErrorKind::MissingQueryClause => "syntax.missing_query_clause",
            ParseErrorKind::MissingSelectOrGroup => "syntax.missing_select_or_group",
            ParseErrorKind::MissingQueryContinuation => "syntax.missing_query_continuation",
            ParseErrorKind::MissingPattern => "syntax.missing_pattern",
            ParseErrorKind::UnclosedPropertyPattern => "syntax.unclosed_property_pattern",
            ParseErrorKind::MissingSubpatternName => "syntax.missing_subpattern_name",
            ParseErrorKind::UnclosedParenthesizedPattern => "syntax.unclosed_parenthesized_pattern",
            ParseErrorKind::UnclosedListPattern => "syntax.unclosed_list_pattern",
        }
    }

    /// The token whose insertion would repair the error, when the kind
    /// says exactly which one and where.
    fn repair(self) -> Option<(&'static str, Placement)> {
        match self {
            ParseErrorKind::MissingSemicolonInUsing => Some((";", Placement::Start)),
            ParseErrorKind::MissingSemicolonInDelegate => Some((";", Placement::Start)),
            ParseErrorKind::MissingSemicolonInMember => Some((";", Placement::Start)),
            ParseErrorKind::MissingSemicolon => Some((";", Placement::Start)),
            ParseErrorKind::MissingColonInSwitchLabel => Some((":", Placement::Start)),
            ParseErrorKind::MissingColonInConditional => Some((":", Placement::Start)),
            ParseErrorKind::UnclosedTypeBody => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedGenericsDefine => Some((">", Placement::End)),
            ParseErrorKind::UnclosedGenericsInfo => Some((">", Placement::End)),
            ParseErrorKind::UnclosedParameterList => Some((")", Placement::End)),
            ParseErrorKind::UnclosedAccessorList => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedAttributeSection => Some(("]", Placement::End)),
            ParseErrorKind::UnclosedArrayRank => Some(("]", Placement::End)),
            ParseErrorKind::UnclosedTupleType => Some((")", Placement::End)),
            ParseErrorKind::UnclosedBlock => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedParenthesis => Some((")", Placement::End)),
            ParseErrorKind::UnclosedSwitchBody => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedArgumentList => Some((")", Placement::End)),
            ParseErrorKind::UnclosedElementAccess => Some(("]", Placement::End)),
            ParseErrorKind::UnclosedInitializer => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedSwitchExpression => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedPropertyPattern => Some(("}", Placement::End)),
            ParseErrorKind::UnclosedParenthesizedPattern => Some((")", Placement::End)),
            ParseErrorKind::UnclosedListPattern => Some(("]", Placement::End)),
            _ => None,
        }
    }
}

impl ParseError {
    pub fn message(&self) -> Message {
        Message::key(self.kind.message_key())
    }

    /// The suggestions that go with the error: for a missing token, the
    /// source with that token added. `source` is the file's text: a token
    /// that was expected *before* the error goes right after the previous
    /// token, not in front of the next line.
    pub fn hints(&self, file: u32, source: &str) -> Vec<Hint> {
        let Some((token, placement)) = self.kind.repair() else {
            return Vec::new();
        };
        let at = match placement {
            Placement::Start => {
                let start = self.span.start.min(source.len());
                source[..start].trim_end().len()
            }
            Placement::End => self.span.end.min(source.len()),
        };
        vec![Hint::edit(
            Message::key("hint.insert_token").arg("token", token),
            Edit::insert(file, at, token),
        )]
    }
}

pub(crate) fn recover_until(
    lexer: &mut Lexer,
    until: &[TokenKind],
    kind: ParseErrorKind,
) -> ParseError {
    let anchor = lexer.cast_anchor();

    loop {
        let token = lexer.current();

        if token.is_none() || until.contains(&token.get_kind()) {
            break;
        }

        lexer.next();
    }

    ParseError {
        kind,
        span: anchor.elapsed(lexer),
    }
}

/// Reports an error at the cursor without consuming anything.
///
/// Used where skipping would do more harm than good -- a missing `;` is best reported
/// where it should have been, leaving the next statement intact.
pub(crate) fn error_here(lexer: &mut Lexer, kind: ParseErrorKind) -> ParseError {
    let span = match lexer.current() {
        Some(token) => token.span,
        None => lexer.cast_anchor().elapsed(lexer),
    };

    ParseError { kind, span }
}

/// Like [`recover_until`], but never stops on a `}` that closes a block opened while
/// skipping. Used where recovery must not escape the construct it started in.
pub(crate) fn recover_until_balanced(
    lexer: &mut Lexer,
    until: &[TokenKind],
    kind: ParseErrorKind,
) -> ParseError {
    let anchor = lexer.cast_anchor();
    let mut depth = 0usize;

    loop {
        let token = lexer.current();

        let Some(token) = token else {
            break;
        };

        if depth == 0 && until.contains(&token.kind) {
            break;
        }

        match token.kind {
            TokenKind::BraceLeft | TokenKind::ParenthesisLeft | TokenKind::BracketLeft => {
                depth += 1;
            }
            TokenKind::BraceRight | TokenKind::ParenthesisRight | TokenKind::BracketRight => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }

        lexer.next();
    }

    ParseError {
        kind,
        span: anchor.elapsed(lexer),
    }
}
