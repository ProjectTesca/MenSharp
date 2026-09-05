use std::ops::Range;

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
