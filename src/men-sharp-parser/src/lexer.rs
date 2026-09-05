use std::ops::Range;

use extension_fn::extension_fn;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    // ==================== trivia ====================
    /// ' ', '\t', ...
    Whitespace,
    /// "\r\n" | "\n" | "\r"
    LineFeed,
    /// `// comment`
    LineComment,
    /// `/* comment */`
    BlockComment,
    /// `/// document`
    LineDocComment,
    /// `/** document */`
    BlockDocComment,
    /// A whole preprocessor line: `#if UNITY_EDITOR`, `#region`, `#pragma warning disable`.
    ///
    /// The lexer only delimits it; [`crate::directive`] gives it structure.
    Directive,

    // ==================== literals ====================
    /// e.g. `0`, `1_000`, `10L`, `0xFFu`, `0b0101`
    IntegerLiteral,
    /// e.g. `1.5`, `1e-3`, `2.0f`, `3m`
    RealLiteral,
    /// e.g. `'a'`, `'\n'`, `'\u0041'`
    CharLiteral,
    /// e.g. `"literal"`
    StringLiteral,
    /// e.g. `@"C:\path"`
    VerbatimStringLiteral,
    /// e.g. `$"x = {x}"`, `$@"..."`, `$"""..."""`
    InterpolatedStringLiteral,
    /// e.g. `"""literal"""`
    RawStringLiteral,

    /// e.g. `name`, `_name`, `@class` (verbatim identifier)
    Identifier,

    // ==================== punctuation ====================
    /// (
    ParenthesisLeft,
    /// )
    ParenthesisRight,
    /// {
    BraceLeft,
    /// }
    BraceRight,
    /// [
    BracketLeft,
    /// ]
    BracketRight,
    /// ;
    Semicolon,
    /// ,
    Comma,
    /// .
    Dot,
    /// ..
    DotDot,
    /// :
    Colon,
    /// ::
    DoubleColon,
    /// ?
    QuestionMark,
    /// ?.
    QuestionDot,
    /// ??
    DoubleQuestion,
    /// ??=
    DoubleQuestionEqual,
    /// ->
    Arrow,
    /// =>
    FatArrow,
    /// +
    Plus,
    /// ++
    DoublePlus,
    /// +=
    PlusEqual,
    /// -
    Minus,
    /// --
    DoubleMinus,
    /// -=
    MinusEqual,
    /// *
    Asterisk,
    /// *=
    AsteriskEqual,
    /// /
    Slash,
    /// /=
    SlashEqual,
    /// %
    Percent,
    /// %=
    PercentEqual,
    /// &
    Ampersand,
    /// &&
    DoubleAmpersand,
    /// &=
    AmpersandEqual,
    /// |
    VerticalLine,
    /// ||
    DoubleVerticalLine,
    /// |=
    VerticalLineEqual,
    /// ^
    Circumflex,
    /// ^=
    CircumflexEqual,
    /// ~
    Tilde,
    /// !
    Exclamation,
    /// !=
    ExclamationEqual,
    /// =
    Equal,
    /// ==
    DoubleEqual,
    /// <
    LessThan,
    /// <=
    LessThanEqual,
    /// <<
    LeftShift,
    /// <<=
    LeftShiftEqual,
    /// `>`
    ///
    /// NOTE: `>>`, `>>=`, `>>>` and `>>>=` are intentionally **not** single tokens.
    /// They are lexed as consecutive [`TokenKind::GreaterThan`] / [`TokenKind::GreaterThanEqual`]
    /// so that nested generics such as `List<List<int>>` stay unambiguous.
    /// The parser re-joins adjacent tokens via [`Token::is_adjacent_to`].
    GreaterThan,
    /// `>=`
    GreaterThanEqual,
    /// #
    Hash,

    // ==================== reserved keywords ====================
    Abstract,
    As,
    Base,
    Bool,
    Break,
    Byte,
    Case,
    Catch,
    Char,
    Checked,
    Class,
    Const,
    Continue,
    Decimal,
    Default,
    Delegate,
    Do,
    Double,
    Else,
    Enum,
    Event,
    Explicit,
    Extern,
    False,
    Finally,
    Fixed,
    Float,
    For,
    Foreach,
    Goto,
    If,
    Implicit,
    In,
    Int,
    Interface,
    Internal,
    Is,
    Lock,
    Long,
    Namespace,
    New,
    Null,
    Object,
    Operator,
    Out,
    Override,
    Params,
    Private,
    Protected,
    Public,
    Readonly,
    Ref,
    Return,
    Sbyte,
    Sealed,
    Short,
    Sizeof,
    Stackalloc,
    Static,
    Str,
    Struct,
    Switch,
    This,
    Throw,
    True,
    Try,
    Typeof,
    Uint,
    Ulong,
    Unchecked,
    Unsafe,
    Ushort,
    Using,
    Virtual,
    Void,
    Volatile,
    While,

    // ==================== contextual keywords ====================
    // These may legally appear as ordinary identifiers.
    // Use `TokenKind::can_be_identifier` where an identifier is expected.
    Add,
    Alias,
    And,
    Args,
    Async,
    Await,
    Dynamic,
    File,
    Get,
    Global,
    Init,
    Managed,
    Nameof,
    Nint,
    Not,
    Notnull,
    Nuint,
    Or,
    Partial,
    Record,
    Remove,
    Required,
    Scoped,
    Set,
    Unmanaged,
    Value,
    Var,
    When,
    Where,
    With,
    Yield,
    // LINQ query expression keywords
    Ascending,
    By,
    Descending,
    Equals,
    From,
    Group,
    Into,
    Join,
    Let,
    On,
    Orderby,
    Select,

    /// A character that no tokenizer accepted.
    UnexpectedCharacter,
    /// Not a real token, returned by [`get_kind`] when the token is `None` (end of input).
    None,
}

/// The tokenizer table.
///
/// Every tokenizer is tried at the current position and the **longest** match wins
/// (maximal munch). On a tie the tokenizer that appears **earlier** in this table wins,
/// which is why keywords are listed before [`TokenKind::Identifier`] and doc comments
/// before ordinary comments.
static TOKENIZERS: &[Tokenizer] = &[
    // ---------- preprocessor ----------
    // Before `Hash`, so a bare `#` on its own line is still a (malformed) directive.
    Tokenizer::LineStart(TokenKind::Directive, scan::directive),
    // ---------- trivia ----------
    // `///` must precede `//`, and `/** */` must precede `/* */` (equal length on ties).
    Tokenizer::Regex(TokenKind::LineDocComment, r"///[^\n\r]*"),
    Tokenizer::Regex(TokenKind::LineComment, r"//[^\n\r]*"),
    Tokenizer::Custom(TokenKind::BlockDocComment, scan::block_doc_comment),
    Tokenizer::Custom(TokenKind::BlockComment, scan::block_comment),
    Tokenizer::Regex(TokenKind::LineFeed, r"\r\n|\n|\r"),
    Tokenizer::Regex(
        TokenKind::Whitespace,
        "[ \t\u{000B}\u{000C}\u{00A0}\u{3000}]+",
    ),
    // ---------- string-like literals ----------
    // Longest match keeps these apart from `@ident` / `$` / `"` punctuation.
    Tokenizer::Custom(
        TokenKind::InterpolatedStringLiteral,
        scan::interpolated_string,
    ),
    Tokenizer::Custom(TokenKind::RawStringLiteral, scan::raw_string),
    Tokenizer::Custom(TokenKind::VerbatimStringLiteral, scan::verbatim_string),
    Tokenizer::Custom(TokenKind::StringLiteral, scan::string),
    Tokenizer::Custom(TokenKind::CharLiteral, scan::char),
    // ---------- numeric literals ----------
    // No sign is consumed here: `a-1` must lex as `a`, `-`, `1`.
    Tokenizer::Regex(
        TokenKind::IntegerLiteral,
        r"0[xX][0-9a-fA-F](_*[0-9a-fA-F])*(([uU][lL]?)|([lL][uU]?))?",
    ),
    Tokenizer::Regex(
        TokenKind::IntegerLiteral,
        r"0[bB][01](_*[01])*(([uU][lL]?)|([lL][uU]?))?",
    ),
    Tokenizer::Regex(
        TokenKind::IntegerLiteral,
        r"[0-9](_*[0-9])*(([uU][lL]?)|([lL][uU]?))?",
    ),
    // `1.5`, `1.5e+3f`
    Tokenizer::Regex(
        TokenKind::RealLiteral,
        r"[0-9](_*[0-9])*\.[0-9](_*[0-9])*([eE][+-]?[0-9](_*[0-9])*)?[fFdDmM]?",
    ),
    // `1e10`, `1E-5d`
    Tokenizer::Regex(
        TokenKind::RealLiteral,
        r"[0-9](_*[0-9])*[eE][+-]?[0-9](_*[0-9])*[fFdDmM]?",
    ),
    // `1f`, `2m`
    Tokenizer::Regex(TokenKind::RealLiteral, r"[0-9](_*[0-9])*[fFdDmM]"),
    // `.5`, `.5f` -- must lose to `..` on `a..b`, and it does (`.` alone never matches here).
    Tokenizer::Regex(
        TokenKind::RealLiteral,
        r"\.[0-9](_*[0-9])*([eE][+-]?[0-9](_*[0-9])*)?[fFdDmM]?",
    ),
    // ---------- punctuation ----------
    Tokenizer::Keyword(TokenKind::ParenthesisLeft, "("),
    Tokenizer::Keyword(TokenKind::ParenthesisRight, ")"),
    Tokenizer::Keyword(TokenKind::BraceLeft, "{"),
    Tokenizer::Keyword(TokenKind::BraceRight, "}"),
    Tokenizer::Keyword(TokenKind::BracketLeft, "["),
    Tokenizer::Keyword(TokenKind::BracketRight, "]"),
    Tokenizer::Keyword(TokenKind::Semicolon, ";"),
    Tokenizer::Keyword(TokenKind::Comma, ","),
    Tokenizer::Keyword(TokenKind::Dot, "."),
    Tokenizer::Keyword(TokenKind::DotDot, ".."),
    Tokenizer::Keyword(TokenKind::Colon, ":"),
    Tokenizer::Keyword(TokenKind::DoubleColon, "::"),
    Tokenizer::Keyword(TokenKind::QuestionMark, "?"),
    Tokenizer::Keyword(TokenKind::QuestionDot, "?."),
    Tokenizer::Keyword(TokenKind::DoubleQuestion, "??"),
    Tokenizer::Keyword(TokenKind::DoubleQuestionEqual, "??="),
    Tokenizer::Keyword(TokenKind::Arrow, "->"),
    Tokenizer::Keyword(TokenKind::FatArrow, "=>"),
    Tokenizer::Keyword(TokenKind::Plus, "+"),
    Tokenizer::Keyword(TokenKind::DoublePlus, "++"),
    Tokenizer::Keyword(TokenKind::PlusEqual, "+="),
    Tokenizer::Keyword(TokenKind::Minus, "-"),
    Tokenizer::Keyword(TokenKind::DoubleMinus, "--"),
    Tokenizer::Keyword(TokenKind::MinusEqual, "-="),
    Tokenizer::Keyword(TokenKind::Asterisk, "*"),
    Tokenizer::Keyword(TokenKind::AsteriskEqual, "*="),
    Tokenizer::Keyword(TokenKind::Slash, "/"),
    Tokenizer::Keyword(TokenKind::SlashEqual, "/="),
    Tokenizer::Keyword(TokenKind::Percent, "%"),
    Tokenizer::Keyword(TokenKind::PercentEqual, "%="),
    Tokenizer::Keyword(TokenKind::Ampersand, "&"),
    Tokenizer::Keyword(TokenKind::DoubleAmpersand, "&&"),
    Tokenizer::Keyword(TokenKind::AmpersandEqual, "&="),
    Tokenizer::Keyword(TokenKind::VerticalLine, "|"),
    Tokenizer::Keyword(TokenKind::DoubleVerticalLine, "||"),
    Tokenizer::Keyword(TokenKind::VerticalLineEqual, "|="),
    Tokenizer::Keyword(TokenKind::Circumflex, "^"),
    Tokenizer::Keyword(TokenKind::CircumflexEqual, "^="),
    Tokenizer::Keyword(TokenKind::Tilde, "~"),
    Tokenizer::Keyword(TokenKind::Exclamation, "!"),
    Tokenizer::Keyword(TokenKind::ExclamationEqual, "!="),
    Tokenizer::Keyword(TokenKind::Equal, "="),
    Tokenizer::Keyword(TokenKind::DoubleEqual, "=="),
    Tokenizer::Keyword(TokenKind::LessThan, "<"),
    Tokenizer::Keyword(TokenKind::LessThanEqual, "<="),
    Tokenizer::Keyword(TokenKind::LeftShift, "<<"),
    Tokenizer::Keyword(TokenKind::LeftShiftEqual, "<<="),
    Tokenizer::Keyword(TokenKind::GreaterThan, ">"),
    Tokenizer::Keyword(TokenKind::GreaterThanEqual, ">="),
    Tokenizer::Keyword(TokenKind::Hash, "#"),
    // ---------- reserved keywords ----------
    Tokenizer::Keyword(TokenKind::Abstract, "abstract"),
    Tokenizer::Keyword(TokenKind::As, "as"),
    Tokenizer::Keyword(TokenKind::Base, "base"),
    Tokenizer::Keyword(TokenKind::Bool, "bool"),
    Tokenizer::Keyword(TokenKind::Break, "break"),
    Tokenizer::Keyword(TokenKind::Byte, "byte"),
    Tokenizer::Keyword(TokenKind::Case, "case"),
    Tokenizer::Keyword(TokenKind::Catch, "catch"),
    Tokenizer::Keyword(TokenKind::Char, "char"),
    Tokenizer::Keyword(TokenKind::Checked, "checked"),
    Tokenizer::Keyword(TokenKind::Class, "class"),
    Tokenizer::Keyword(TokenKind::Const, "const"),
    Tokenizer::Keyword(TokenKind::Continue, "continue"),
    Tokenizer::Keyword(TokenKind::Decimal, "decimal"),
    Tokenizer::Keyword(TokenKind::Default, "default"),
    Tokenizer::Keyword(TokenKind::Delegate, "delegate"),
    Tokenizer::Keyword(TokenKind::Do, "do"),
    Tokenizer::Keyword(TokenKind::Double, "double"),
    Tokenizer::Keyword(TokenKind::Else, "else"),
    Tokenizer::Keyword(TokenKind::Enum, "enum"),
    Tokenizer::Keyword(TokenKind::Event, "event"),
    Tokenizer::Keyword(TokenKind::Explicit, "explicit"),
    Tokenizer::Keyword(TokenKind::Extern, "extern"),
    Tokenizer::Keyword(TokenKind::False, "false"),
    Tokenizer::Keyword(TokenKind::Finally, "finally"),
    Tokenizer::Keyword(TokenKind::Fixed, "fixed"),
    Tokenizer::Keyword(TokenKind::Float, "float"),
    Tokenizer::Keyword(TokenKind::For, "for"),
    Tokenizer::Keyword(TokenKind::Foreach, "foreach"),
    Tokenizer::Keyword(TokenKind::Goto, "goto"),
    Tokenizer::Keyword(TokenKind::If, "if"),
    Tokenizer::Keyword(TokenKind::Implicit, "implicit"),
    Tokenizer::Keyword(TokenKind::In, "in"),
    Tokenizer::Keyword(TokenKind::Int, "int"),
    Tokenizer::Keyword(TokenKind::Interface, "interface"),
    Tokenizer::Keyword(TokenKind::Internal, "internal"),
    Tokenizer::Keyword(TokenKind::Is, "is"),
    Tokenizer::Keyword(TokenKind::Lock, "lock"),
    Tokenizer::Keyword(TokenKind::Long, "long"),
    Tokenizer::Keyword(TokenKind::Namespace, "namespace"),
    Tokenizer::Keyword(TokenKind::New, "new"),
    Tokenizer::Keyword(TokenKind::Null, "null"),
    Tokenizer::Keyword(TokenKind::Object, "object"),
    Tokenizer::Keyword(TokenKind::Operator, "operator"),
    Tokenizer::Keyword(TokenKind::Out, "out"),
    Tokenizer::Keyword(TokenKind::Override, "override"),
    Tokenizer::Keyword(TokenKind::Params, "params"),
    Tokenizer::Keyword(TokenKind::Private, "private"),
    Tokenizer::Keyword(TokenKind::Protected, "protected"),
    Tokenizer::Keyword(TokenKind::Public, "public"),
    Tokenizer::Keyword(TokenKind::Readonly, "readonly"),
    Tokenizer::Keyword(TokenKind::Ref, "ref"),
    Tokenizer::Keyword(TokenKind::Return, "return"),
    Tokenizer::Keyword(TokenKind::Sbyte, "sbyte"),
    Tokenizer::Keyword(TokenKind::Sealed, "sealed"),
    Tokenizer::Keyword(TokenKind::Short, "short"),
    Tokenizer::Keyword(TokenKind::Sizeof, "sizeof"),
    Tokenizer::Keyword(TokenKind::Stackalloc, "stackalloc"),
    Tokenizer::Keyword(TokenKind::Static, "static"),
    Tokenizer::Keyword(TokenKind::Str, "string"),
    Tokenizer::Keyword(TokenKind::Struct, "struct"),
    Tokenizer::Keyword(TokenKind::Switch, "switch"),
    Tokenizer::Keyword(TokenKind::This, "this"),
    Tokenizer::Keyword(TokenKind::Throw, "throw"),
    Tokenizer::Keyword(TokenKind::True, "true"),
    Tokenizer::Keyword(TokenKind::Try, "try"),
    Tokenizer::Keyword(TokenKind::Typeof, "typeof"),
    Tokenizer::Keyword(TokenKind::Uint, "uint"),
    Tokenizer::Keyword(TokenKind::Ulong, "ulong"),
    Tokenizer::Keyword(TokenKind::Unchecked, "unchecked"),
    Tokenizer::Keyword(TokenKind::Unsafe, "unsafe"),
    Tokenizer::Keyword(TokenKind::Ushort, "ushort"),
    Tokenizer::Keyword(TokenKind::Using, "using"),
    Tokenizer::Keyword(TokenKind::Virtual, "virtual"),
    Tokenizer::Keyword(TokenKind::Void, "void"),
    Tokenizer::Keyword(TokenKind::Volatile, "volatile"),
    Tokenizer::Keyword(TokenKind::While, "while"),
    // ---------- contextual keywords ----------
    Tokenizer::Keyword(TokenKind::Add, "add"),
    Tokenizer::Keyword(TokenKind::Alias, "alias"),
    Tokenizer::Keyword(TokenKind::And, "and"),
    Tokenizer::Keyword(TokenKind::Args, "args"),
    Tokenizer::Keyword(TokenKind::Async, "async"),
    Tokenizer::Keyword(TokenKind::Await, "await"),
    Tokenizer::Keyword(TokenKind::Dynamic, "dynamic"),
    Tokenizer::Keyword(TokenKind::File, "file"),
    Tokenizer::Keyword(TokenKind::Get, "get"),
    Tokenizer::Keyword(TokenKind::Global, "global"),
    Tokenizer::Keyword(TokenKind::Init, "init"),
    Tokenizer::Keyword(TokenKind::Managed, "managed"),
    Tokenizer::Keyword(TokenKind::Nameof, "nameof"),
    Tokenizer::Keyword(TokenKind::Nint, "nint"),
    Tokenizer::Keyword(TokenKind::Not, "not"),
    Tokenizer::Keyword(TokenKind::Notnull, "notnull"),
    Tokenizer::Keyword(TokenKind::Nuint, "nuint"),
    Tokenizer::Keyword(TokenKind::Or, "or"),
    Tokenizer::Keyword(TokenKind::Partial, "partial"),
    Tokenizer::Keyword(TokenKind::Record, "record"),
    Tokenizer::Keyword(TokenKind::Remove, "remove"),
    Tokenizer::Keyword(TokenKind::Required, "required"),
    Tokenizer::Keyword(TokenKind::Scoped, "scoped"),
    Tokenizer::Keyword(TokenKind::Set, "set"),
    Tokenizer::Keyword(TokenKind::Unmanaged, "unmanaged"),
    Tokenizer::Keyword(TokenKind::Value, "value"),
    Tokenizer::Keyword(TokenKind::Var, "var"),
    Tokenizer::Keyword(TokenKind::When, "when"),
    Tokenizer::Keyword(TokenKind::Where, "where"),
    Tokenizer::Keyword(TokenKind::With, "with"),
    Tokenizer::Keyword(TokenKind::Yield, "yield"),
    Tokenizer::Keyword(TokenKind::Ascending, "ascending"),
    Tokenizer::Keyword(TokenKind::By, "by"),
    Tokenizer::Keyword(TokenKind::Descending, "descending"),
    Tokenizer::Keyword(TokenKind::Equals, "equals"),
    Tokenizer::Keyword(TokenKind::From, "from"),
    Tokenizer::Keyword(TokenKind::Group, "group"),
    Tokenizer::Keyword(TokenKind::Into, "into"),
    Tokenizer::Keyword(TokenKind::Join, "join"),
    Tokenizer::Keyword(TokenKind::Let, "let"),
    Tokenizer::Keyword(TokenKind::On, "on"),
    Tokenizer::Keyword(TokenKind::Orderby, "orderby"),
    Tokenizer::Keyword(TokenKind::Select, "select"),
    // ---------- identifier ----------
    // Must come after every keyword: on an exact-length tie the keyword wins,
    // while `interfaces` still lexes as one identifier (longest match).
    Tokenizer::Regex(
        TokenKind::Identifier,
        r"@?[_\p{L}\p{Nl}][_\p{L}\p{Nl}\p{Mn}\p{Mc}\p{Nd}\p{Cf}]*",
    ),
];

impl TokenKind {
    /// The fixed spelling of a punctuation or keyword token, if it has one.
    pub fn fixed_text(&self) -> Option<&'static str> {
        TOKENIZERS.iter().find_map(|tokenizer| match tokenizer {
            Tokenizer::Keyword(kind, keyword) if kind == self => Some(*keyword),
            _ => None,
        })
    }

    /// Whitespace, line feeds and comments.
    pub fn is_trivia(&self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace
                | TokenKind::LineFeed
                | TokenKind::LineComment
                | TokenKind::BlockComment
                | TokenKind::LineDocComment
                | TokenKind::BlockDocComment
        )
    }

    pub fn is_comment(&self) -> bool {
        matches!(
            self,
            TokenKind::LineComment
                | TokenKind::BlockComment
                | TokenKind::LineDocComment
                | TokenKind::BlockDocComment
        )
    }

    pub fn is_doc_comment(&self) -> bool {
        matches!(self, TokenKind::LineDocComment | TokenKind::BlockDocComment)
    }

    pub fn is_literal(&self) -> bool {
        matches!(
            self,
            TokenKind::IntegerLiteral
                | TokenKind::RealLiteral
                | TokenKind::CharLiteral
                | TokenKind::StringLiteral
                | TokenKind::VerbatimStringLiteral
                | TokenKind::InterpolatedStringLiteral
                | TokenKind::RawStringLiteral
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
        )
    }

    pub fn is_string_literal(&self) -> bool {
        matches!(
            self,
            TokenKind::StringLiteral
                | TokenKind::VerbatimStringLiteral
                | TokenKind::InterpolatedStringLiteral
                | TokenKind::RawStringLiteral
        )
    }

    /// One of the built-in value/reference type keywords (`int`, `string`, ...).
    pub fn is_predefined_type(&self) -> bool {
        matches!(
            self,
            TokenKind::Bool
                | TokenKind::Byte
                | TokenKind::Char
                | TokenKind::Decimal
                | TokenKind::Double
                | TokenKind::Float
                | TokenKind::Int
                | TokenKind::Long
                | TokenKind::Object
                | TokenKind::Sbyte
                | TokenKind::Short
                | TokenKind::Str
                | TokenKind::Uint
                | TokenKind::Ulong
                | TokenKind::Ushort
                | TokenKind::Void
                | TokenKind::Nint
                | TokenKind::Nuint
                | TokenKind::Dynamic
        )
    }

    /// A keyword that can never be used as a plain identifier.
    ///
    /// Relies on the reserved keywords being declared contiguously,
    /// from [`TokenKind::Abstract`] to [`TokenKind::While`].
    pub fn is_reserved_keyword(&self) -> bool {
        (TokenKind::Abstract as u32..=TokenKind::While as u32).contains(&(*self as u32))
    }

    /// A keyword that is only a keyword in certain syntactic positions,
    /// and is otherwise a valid identifier.
    ///
    /// Relies on the contextual keywords being declared contiguously,
    /// from [`TokenKind::Add`] to [`TokenKind::Select`].
    pub fn is_contextual_keyword(&self) -> bool {
        (TokenKind::Add as u32..=TokenKind::Select as u32).contains(&(*self as u32))
    }

    pub fn is_keyword(&self) -> bool {
        self.is_reserved_keyword() || self.is_contextual_keyword()
    }

    /// Whether this token may stand where the grammar expects an identifier.
    pub fn can_be_identifier(&self) -> bool {
        matches!(self, TokenKind::Identifier) || self.is_contextual_keyword()
    }
}

enum Tokenizer {
    /// Matches a literal string.
    Keyword(TokenKind, &'static str),
    /// Matches an anchored regular expression. Compiled once per process
    /// (see [`compiled_regexes`]): compiling the table again for every file
    /// cost more than lexing the file did.
    Regex(TokenKind, &'static str),
    /// Matches with a hand written scanner. Returns the accepted byte length, or 0 to reject.
    Custom(TokenKind, fn(&str) -> usize),
    /// Like [`Tokenizer::Custom`], but only when nothing except whitespace precedes the
    /// cursor on its line. C# requires that of preprocessor directives.
    LineStart(TokenKind, fn(&str) -> usize),
}

/// The regular expressions of [`TOKENIZERS`], compiled once for the whole
/// process and indexed like the table (`None` where the tokenizer is not a
/// regex).
fn compiled_regexes() -> &'static [Option<Regex>] {
    static COMPILED: std::sync::OnceLock<Vec<Option<Regex>>> = std::sync::OnceLock::new();
    COMPILED.get_or_init(|| {
        TOKENIZERS
            .iter()
            .map(|tokenizer| match tokenizer {
                Tokenizer::Regex(_, regex) => {
                    Some(Regex::new(format!("^({regex})").as_str()).unwrap())
                }
                _ => None,
            })
            .collect()
    })
}

impl Tokenizer {
    fn tokenize(
        &self,
        current_input: &str,
        index: usize,
        regexes: &[Option<Regex>],
        at_line_start: bool,
    ) -> (TokenKind, usize) {
        match self {
            Tokenizer::Keyword(kind, keyword) => {
                if current_input.starts_with(keyword) {
                    (*kind, keyword.len())
                } else {
                    (*kind, 0)
                }
            }
            Tokenizer::Regex(kind, _) => {
                let regex = regexes[index]
                    .as_ref()
                    .expect("every Regex tokenizer is compiled by compiled_regexes");

                let length = match regex.find(current_input) {
                    Some(matched) => matched.end(),
                    None => 0,
                };

                (*kind, length)
            }
            Tokenizer::Custom(kind, scanner) => (*kind, scanner(current_input)),
            Tokenizer::LineStart(kind, scanner) => (
                *kind,
                if at_line_start {
                    scanner(current_input)
                } else {
                    0
                },
            ),
        }
    }
}

/// Hand written scanners for constructs a regular expression cannot express:
/// nesting (`$"{ f("x") }"`), paired delimiters of variable width (`"""`),
/// and non-greedy termination (`/* */`).
///
/// Every scanner returns the accepted byte length, or 0 if it does not match at all.
/// An unterminated literal or comment is still accepted, consuming up to the point
/// where the construct must have ended (end of line, or end of file).
/// Callers can detect this by checking whether the token text ends with its closing
/// delimiter -- see [`Token::is_terminated`].
pub(crate) mod scan {
    /// Length in bytes of the UTF-8 sequence that starts with `byte`.
    fn utf8_length(byte: u8) -> usize {
        match byte {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            // continuation byte or invalid: advance one byte to guarantee progress
            _ => 1,
        }
    }

    /// A preprocessor directive: `#` up to, but not including, the line terminator.
    pub fn directive(input: &str) -> usize {
        if !input.starts_with('#') {
            return 0;
        }

        input.find(['\n', '\r']).unwrap_or(input.len())
    }

    /// `/* ... */`, terminated by the **first** `*/`.
    pub fn block_comment(input: &str) -> usize {
        if !input.starts_with("/*") {
            return 0;
        }

        let bytes = input.as_bytes();
        let mut index = 2;
        while index + 1 < bytes.len() {
            if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                return index + 2;
            }
            index += 1;
        }

        bytes.len() // unterminated
    }

    /// `/** ... */`. `/**/` is an ordinary (empty) block comment, not a doc comment.
    pub fn block_doc_comment(input: &str) -> usize {
        if !input.starts_with("/**") || input.as_bytes().get(3) == Some(&b'/') {
            return 0;
        }

        block_comment(input)
    }

    /// `"..."`, with `\` escapes. Never spans a line break.
    pub fn string(input: &str) -> usize {
        if !input.starts_with('"') {
            return 0;
        }
        // `"""` is a raw string literal, not an empty string followed by a quote.
        if input.starts_with(r#"""""#) {
            return 0;
        }

        let bytes = input.as_bytes();
        let mut index = 1;
        while index < bytes.len() {
            match bytes[index] {
                b'"' => return index + 1,
                b'\\' => {
                    index += 1;
                    if index < bytes.len() {
                        index += utf8_length(bytes[index]);
                    }
                }
                b'\n' | b'\r' => return index, // unterminated
                byte => index += utf8_length(byte),
            }
        }

        bytes.len() // unterminated
    }

    /// `'c'`, with `\` escapes. Never spans a line break.
    pub fn char(input: &str) -> usize {
        if !input.starts_with('\'') {
            return 0;
        }

        let bytes = input.as_bytes();
        let mut index = 1;
        while index < bytes.len() {
            match bytes[index] {
                b'\'' => return index + 1,
                b'\\' => {
                    index += 1;
                    if index < bytes.len() {
                        index += utf8_length(bytes[index]);
                    }
                }
                b'\n' | b'\r' => return index, // unterminated
                byte => index += utf8_length(byte),
            }
        }

        bytes.len() // unterminated
    }

    /// `@"..."`, where `""` is an escaped quote. May span line breaks.
    pub fn verbatim_string(input: &str) -> usize {
        if !input.starts_with("@\"") {
            return 0;
        }

        let bytes = input.as_bytes();
        let mut index = 2;
        while index < bytes.len() {
            if bytes[index] == b'"' {
                if bytes.get(index + 1) == Some(&b'"') {
                    index += 2;
                    continue;
                }
                return index + 1;
            }
            index += utf8_length(bytes[index]);
        }

        bytes.len() // unterminated
    }

    /// `"""..."""` (three or more quotes), closed by a quote run of at least the same width.
    pub fn raw_string(input: &str) -> usize {
        if !input.starts_with(r#"""""#) {
            return 0;
        }

        let bytes = input.as_bytes();
        let mut index = 0;
        while bytes.get(index) == Some(&b'"') {
            index += 1;
        }

        raw_string_body(bytes, index, index)
    }

    /// Scans from `index` to the end of a raw string whose opening quote run was `quote_count` wide.
    fn raw_string_body(bytes: &[u8], mut index: usize, quote_count: usize) -> usize {
        while index < bytes.len() {
            if bytes[index] != b'"' {
                index += utf8_length(bytes[index]);
                continue;
            }

            let run_start = index;
            while bytes.get(index) == Some(&b'"') {
                index += 1;
            }
            if index - run_start >= quote_count {
                return index;
            }
        }

        bytes.len() // unterminated
    }

    /// `$"..."`, `$@"..."`, `@$"..."` and `$"""..."""`.
    ///
    /// Interpolation holes are tracked by brace depth, and literals/comments nested
    /// inside a hole are skipped recursively, so `$"{ f("}") }"` lexes as one token.
    pub fn interpolated_string(input: &str) -> usize {
        let bytes = input.as_bytes();

        let mut index = 0;
        let mut has_dollar = false;
        let mut is_verbatim = false;
        while index < bytes.len() && (bytes[index] == b'$' || bytes[index] == b'@') {
            if bytes[index] == b'$' {
                has_dollar = true;
            } else {
                is_verbatim = true;
            }
            index += 1;
        }

        if !has_dollar || bytes.get(index) != Some(&b'"') {
            return 0;
        }

        let quote_start = index;
        while bytes.get(index) == Some(&b'"') {
            index += 1;
        }
        let quote_count = index - quote_start;

        if quote_count >= 3 {
            return raw_string_body(bytes, index, quote_count);
        }
        if quote_count == 2 {
            return index; // `$""`
        }

        let mut brace_depth = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];

            if brace_depth == 0 {
                match byte {
                    b'"' => {
                        // in a verbatim interpolated string `""` is an escaped quote
                        if is_verbatim && bytes.get(index + 1) == Some(&b'"') {
                            index += 2;
                            continue;
                        }
                        return index + 1;
                    }
                    b'\\' if !is_verbatim => {
                        index += 1;
                        if index < bytes.len() {
                            index += utf8_length(bytes[index]);
                        }
                    }
                    b'{' | b'}' => {
                        // `{{` and `}}` are escaped braces
                        if bytes.get(index + 1) == Some(&byte) {
                            index += 2;
                            continue;
                        }
                        if byte == b'{' {
                            brace_depth = 1;
                        }
                        index += 1;
                    }
                    b'\n' | b'\r' if !is_verbatim => return index, // unterminated
                    byte => index += utf8_length(byte),
                }
                continue;
            }

            // inside an interpolation hole: plain C# code
            match byte {
                b'{' => {
                    brace_depth += 1;
                    index += 1;
                }
                b'}' => {
                    brace_depth -= 1;
                    index += 1;
                }
                b'"' | b'\'' | b'$' | b'@' | b'/' => {
                    let nested = literal_or_comment_length(&input[index..]);
                    index += if nested == 0 { 1 } else { nested };
                }
                byte => index += utf8_length(byte),
            }
        }

        bytes.len() // unterminated
    }

    /// Length of a literal or comment starting at the front of `input`, or 0 if there is none.
    /// Used to skip over constructs nested inside an interpolation hole.
    pub(crate) fn literal_or_comment_length(input: &str) -> usize {
        [
            interpolated_string as fn(&str) -> usize,
            raw_string,
            verbatim_string,
            string,
            char,
            block_comment,
            // A line comment cannot legally appear inside a single line hole, but skipping
            // it keeps an unterminated string from swallowing the rest of the file.
            |input: &str| {
                if input.starts_with("//") {
                    input.find(['\n', '\r']).unwrap_or(input.len())
                } else {
                    0
                }
            },
        ]
        .into_iter()
        .map(|scanner| scanner(input))
        .max()
        .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token<'input> {
    pub kind: TokenKind,
    pub text: &'input str,
    pub span: Range<usize>,
}

impl<'input> Token<'input> {
    /// Whether `other` starts exactly where this token ends, with no trivia between.
    ///
    /// The parser uses this to re-join the deliberately split `>` tokens into
    /// `>>`, `>>=`, `>>>` and `>>>=`.
    pub fn is_adjacent_to(&self, other: &Token<'_>) -> bool {
        self.span.end == other.span.start
    }

    /// Whether a literal or block comment reached its closing delimiter.
    /// Always true for tokens that have no closing delimiter.
    pub fn is_terminated(&self) -> bool {
        let closing = match self.kind {
            TokenKind::CharLiteral => "'",
            TokenKind::StringLiteral
            | TokenKind::VerbatimStringLiteral
            | TokenKind::InterpolatedStringLiteral
            | TokenKind::RawStringLiteral => "\"",
            TokenKind::BlockComment | TokenKind::BlockDocComment => "*/",
            _ => return true,
        };

        // the opening delimiter alone must not count as the closing one
        self.text.len() > closing.len() && self.text.ends_with(closing)
    }
}

#[extension_fn(Option<Token<'_>>)]
pub fn get_kind(&self) -> TokenKind {
    self.as_ref()
        .map(|token| token.kind)
        .unwrap_or(TokenKind::None)
}

pub struct Lexer<'input> {
    source: &'input str,
    current_byte_position: usize,
    current_token_cache: Option<Token<'input>>,
    /// Comments already recorded in [`Lexer::comments`], so that re-lexing the same
    /// region (via [`Lexer::current`] or [`Lexer::back_to_anchor`]) does not duplicate them.
    comment_scan_position: usize,
    /// Every comment passed over while it was set to be ignored, in source order.
    pub comments: Vec<Token<'input>>,
    pub ignore_whitespace: bool,
    /// C# is not line sensitive, so line feeds are skipped by default.
    pub ignore_line_feed: bool,
    pub ignore_comment: bool,
    /// Doc comments are surfaced as tokens by default so the parser can attach them
    /// to the declaration that follows.
    pub ignore_doc_comment: bool,
    /// Preprocessor directives are trivia to the parser: they can appear between any two
    /// tokens, so threading them through the grammar would touch every rule for no gain.
    /// They are collected here instead, in source order, for a preprocessing pass.
    pub ignore_directive: bool,
    /// Every directive passed over while `ignore_directive` was set.
    pub directives: Vec<Token<'input>>,
    /// Directives already recorded, so re-lexing does not duplicate them.
    directive_scan_position: usize,
    /// Added to every span this lexer reports.
    ///
    /// A sub lexer over part of a file -- the inside of an interpolation hole, say --
    /// works on a `&str` that starts at 0, but every span it produces has to point back
    /// into the original source. This is where that region began.
    span_offset: usize,
    /// How many `unsafe` scopes enclose the cursor.
    ///
    /// This is parser state, not lexing state, but the lexer is the one object every rule
    /// already threads through, and `T*` cannot be decided without it: outside an `unsafe`
    /// scope `a * b;` is a multiplication, inside one it is a pointer declaration. C# draws
    /// the line in exactly the same place.
    pub unsafe_depth: usize,
}

impl<'input> Lexer<'input> {
    pub fn new(source: &'input str) -> Self {
        Self::new_at(source, 0)
    }

    /// A lexer over a slice of a larger file, reporting spans in the file's coordinates.
    pub fn new_at(source: &'input str, span_offset: usize) -> Self {
        Self {
            span_offset,
            source,
            current_byte_position: 0,
            current_token_cache: None,
            comment_scan_position: 0,
            comments: Vec::new(),
            ignore_whitespace: true,
            ignore_line_feed: true,
            ignore_comment: true,
            ignore_doc_comment: false,
            ignore_directive: true,
            directives: Vec::new(),
            directive_scan_position: 0,
            unsafe_depth: 0,
        }
    }

    /// Whether only whitespace stands between the cursor and the start of its line.
    fn at_line_start(&self) -> bool {
        self.source[..self.current_byte_position]
            .chars()
            .rev()
            .find(|char| !matches!(char, ' ' | '\t' | '\u{000B}' | '\u{000C}' | '\u{3000}'))
            .map(|char| char == '\n' || char == '\r')
            .unwrap_or(true)
    }

    pub fn current(&mut self) -> Option<Token<'input>> {
        let anchor = self.cast_anchor();

        // move to next temporarily
        self.current_token_cache = self.next();

        // back to anchor position
        self.current_byte_position = anchor.byte_position;

        self.current_token_cache.clone()
    }

    pub fn cast_anchor(&self) -> Anchor {
        Anchor {
            byte_position: self.current_byte_position,
        }
    }

    pub fn skip_line_feed(&mut self) {
        while let TokenKind::LineFeed = self.current().get_kind() {
            self.next();
        }
    }

    pub fn back_to_anchor(&mut self, anchor: Anchor) {
        self.current_byte_position = anchor.byte_position;
        self.current_token_cache = None;
    }

    pub fn enable_comment_token(mut self) -> Self {
        self.ignore_comment = false;
        self.ignore_doc_comment = false;
        self
    }

    pub fn disable_doc_comment_token(mut self) -> Self {
        self.ignore_doc_comment = true;
        self
    }

    pub fn enable_whitespace_token(mut self) -> Self {
        self.ignore_whitespace = false;
        self
    }

    pub fn enable_line_feed_token(mut self) -> Self {
        self.ignore_line_feed = false;
        self
    }

    pub fn enable_directive_token(mut self) -> Self {
        self.ignore_directive = false;
        self
    }
}

impl<'input> Iterator for Lexer<'input> {
    type Item = Token<'input>;

    fn next(&mut self) -> Option<Self::Item> {
        // take cache
        if let Some(token) = self.current_token_cache.take() {
            // spans are reported in the file's coordinates; positions are local
            self.current_byte_position = token.span.end - self.span_offset;
            return Some(token);
        }

        loop {
            if self.current_byte_position == self.source.len() {
                return None;
            }

            let current_input = &self.source[self.current_byte_position..self.source.len()];

            let mut current_max_length = 0;
            let mut current_token_kind = TokenKind::UnexpectedCharacter;

            let at_line_start = self.at_line_start();

            for (index, tokenizer) in TOKENIZERS.iter().enumerate() {
                let (token_kind, byte_length) =
                    tokenizer.tokenize(current_input, index, compiled_regexes(), at_line_start);

                if byte_length > current_max_length {
                    current_max_length = byte_length;
                    current_token_kind = token_kind;
                }
            }

            let start_position = self.current_byte_position;

            let token = if current_max_length == 0 {
                let char_length = self.source[start_position..]
                    .chars()
                    .next()
                    .unwrap()
                    .len_utf8();

                self.current_byte_position += char_length;
                let end_position = start_position + char_length;

                Token {
                    kind: TokenKind::UnexpectedCharacter,
                    text: &self.source[start_position..end_position],
                    span: self.span_offset + start_position..self.span_offset + end_position,
                }
            } else {
                self.current_byte_position += current_max_length;
                let end_position = self.current_byte_position;

                let token = Token {
                    kind: current_token_kind,
                    text: &self.source[start_position..end_position],
                    span: self.span_offset + start_position..self.span_offset + end_position,
                };

                if current_token_kind == TokenKind::Directive && self.ignore_directive {
                    if start_position >= self.directive_scan_position {
                        self.directive_scan_position = end_position;
                        self.directives.push(token);
                    }
                    continue;
                }

                if current_token_kind.is_comment() {
                    let ignore = if current_token_kind.is_doc_comment() {
                        self.ignore_doc_comment
                    } else {
                        self.ignore_comment
                    };

                    if ignore {
                        if start_position >= self.comment_scan_position {
                            self.comment_scan_position = end_position;
                            self.comments.push(token);
                        }
                        continue;
                    }
                }

                match current_token_kind {
                    TokenKind::Whitespace if self.ignore_whitespace => continue,
                    TokenKind::LineFeed if self.ignore_line_feed => continue,
                    _ => {}
                }

                token
            };

            return Some(token);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    byte_position: usize,
}

impl Anchor {
    pub fn elapsed(&self, lexer: &Lexer) -> Range<usize> {
        // skip until not whitespace
        let floor = lexer.source[self.byte_position..]
            .chars()
            .take_while(|char| char.is_whitespace())
            .map(|char| char.len_utf8())
            .sum::<usize>();

        let start = self.byte_position + floor;
        let end = lexer.current_byte_position.max(start);

        lexer.span_offset + start..lexer.span_offset + end
    }
}

#[cfg(test)]
mod tests {
    use super::{Lexer, TokenKind};

    fn lex(source: &str) -> Vec<(TokenKind, &str)> {
        Lexer::new(source)
            .map(|token| (token.kind, token.text))
            .collect()
    }

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source).into_iter().map(|(kind, _)| kind).collect()
    }

    #[test]
    fn keyword_wins_over_identifier_on_a_tie() {
        assert_eq!(kinds("int"), vec![TokenKind::Int]);
    }

    #[test]
    fn identifier_wins_when_it_is_longer() {
        assert_eq!(
            lex("interfaces"),
            vec![(TokenKind::Identifier, "interfaces")]
        );
        assert_eq!(lex("classy"), vec![(TokenKind::Identifier, "classy")]);
        assert_eq!(lex("_new"), vec![(TokenKind::Identifier, "_new")]);
        assert_eq!(lex("@class"), vec![(TokenKind::Identifier, "@class")]);
        assert_eq!(lex("名前"), vec![(TokenKind::Identifier, "名前")]);
    }

    #[test]
    fn declaration() {
        assert_eq!(
            lex("public static int Main() { }"),
            vec![
                (TokenKind::Public, "public"),
                (TokenKind::Static, "static"),
                (TokenKind::Int, "int"),
                (TokenKind::Identifier, "Main"),
                (TokenKind::ParenthesisLeft, "("),
                (TokenKind::ParenthesisRight, ")"),
                (TokenKind::BraceLeft, "{"),
                (TokenKind::BraceRight, "}"),
            ]
        );
    }

    #[test]
    fn operators_use_maximal_munch() {
        assert_eq!(
            kinds("a ??= b"),
            vec![
                TokenKind::Identifier,
                TokenKind::DoubleQuestionEqual,
                TokenKind::Identifier
            ]
        );
        assert_eq!(
            kinds("a <<= 1"),
            vec![
                TokenKind::Identifier,
                TokenKind::LeftShiftEqual,
                TokenKind::IntegerLiteral
            ]
        );
        assert_eq!(
            kinds("a?.b"),
            vec![
                TokenKind::Identifier,
                TokenKind::QuestionDot,
                TokenKind::Identifier
            ]
        );
        assert_eq!(
            kinds("x => x++"),
            vec![
                TokenKind::Identifier,
                TokenKind::FatArrow,
                TokenKind::Identifier,
                TokenKind::DoublePlus
            ]
        );
    }

    #[test]
    fn signs_are_not_part_of_numbers() {
        assert_eq!(
            lex("a-1"),
            vec![
                (TokenKind::Identifier, "a"),
                (TokenKind::Minus, "-"),
                (TokenKind::IntegerLiteral, "1"),
            ]
        );
    }

    #[test]
    fn numeric_literals() {
        assert_eq!(lex("0xFFu"), vec![(TokenKind::IntegerLiteral, "0xFFu")]);
        assert_eq!(lex("0xF_Fu"), vec![(TokenKind::IntegerLiteral, "0xF_Fu")]);
        // a digit separator may not sit next to the suffix, so `_u` is not part of the literal
        assert_eq!(
            lex("0xFF_u"),
            vec![
                (TokenKind::IntegerLiteral, "0xFF"),
                (TokenKind::Identifier, "_u"),
            ]
        );
        assert_eq!(lex("0b1010"), vec![(TokenKind::IntegerLiteral, "0b1010")]);
        assert_eq!(lex("1_000L"), vec![(TokenKind::IntegerLiteral, "1_000L")]);
        assert_eq!(lex("1.5"), vec![(TokenKind::RealLiteral, "1.5")]);
        assert_eq!(lex("1.5e+3f"), vec![(TokenKind::RealLiteral, "1.5e+3f")]);
        assert_eq!(lex("1e10"), vec![(TokenKind::RealLiteral, "1e10")]);
        assert_eq!(lex("2f"), vec![(TokenKind::RealLiteral, "2f")]);
        assert_eq!(lex(".5f"), vec![(TokenKind::RealLiteral, ".5f")]);
    }

    #[test]
    fn range_operator_beats_real_literal() {
        assert_eq!(
            lex("1..2"),
            vec![
                (TokenKind::IntegerLiteral, "1"),
                (TokenKind::DotDot, ".."),
                (TokenKind::IntegerLiteral, "2"),
            ]
        );
    }

    #[test]
    fn member_access_on_a_number_is_not_a_real_literal() {
        assert_eq!(
            lex("1.ToString()"),
            vec![
                (TokenKind::IntegerLiteral, "1"),
                (TokenKind::Dot, "."),
                (TokenKind::Identifier, "ToString"),
                (TokenKind::ParenthesisLeft, "("),
                (TokenKind::ParenthesisRight, ")"),
            ]
        );
    }

    #[test]
    fn nested_generics_do_not_produce_a_right_shift() {
        assert_eq!(
            kinds("List<List<int>>"),
            vec![
                TokenKind::Identifier,
                TokenKind::LessThan,
                TokenKind::Identifier,
                TokenKind::LessThan,
                TokenKind::Int,
                TokenKind::GreaterThan,
                TokenKind::GreaterThan,
            ]
        );

        let tokens: Vec<_> = Lexer::new("a >> b").collect();
        assert_eq!(tokens[1].kind, TokenKind::GreaterThan);
        assert_eq!(tokens[2].kind, TokenKind::GreaterThan);
        assert!(tokens[1].is_adjacent_to(&tokens[2]));

        let tokens: Vec<_> = Lexer::new("a > > b").collect();
        assert!(!tokens[1].is_adjacent_to(&tokens[2]));
    }

    #[test]
    fn string_literals() {
        assert_eq!(
            lex(r#""abc""#),
            vec![(TokenKind::StringLiteral, r#""abc""#)]
        );
        assert_eq!(
            lex(r#""a\"b""#),
            vec![(TokenKind::StringLiteral, r#""a\"b""#)]
        );
        assert_eq!(lex(r#""""#), vec![(TokenKind::StringLiteral, r#""""#)]);
        assert_eq!(
            lex(r#"@"C:\path""#),
            vec![(TokenKind::VerbatimStringLiteral, r#"@"C:\path""#)]
        );
        assert_eq!(
            lex(r#"@"a""b""#),
            vec![(TokenKind::VerbatimStringLiteral, r#"@"a""b""#)]
        );
        assert_eq!(
            lex(r#""""a "" b""""#),
            vec![(TokenKind::RawStringLiteral, r#""""a "" b""""#)]
        );
    }

    #[test]
    fn interpolated_strings_track_nesting() {
        assert_eq!(
            lex(r#"$"x = {x}""#),
            vec![(TokenKind::InterpolatedStringLiteral, r#"$"x = {x}""#)]
        );
        // a quote inside a hole must not close the string
        assert_eq!(
            lex(r#"$"{f("}")}""#),
            vec![(TokenKind::InterpolatedStringLiteral, r#"$"{f("}")}""#)]
        );
        // escaped braces
        assert_eq!(
            lex(r#"$"{{literal}}""#),
            vec![(TokenKind::InterpolatedStringLiteral, r#"$"{{literal}}""#)]
        );
        assert_eq!(
            lex(r#"$@"{a}\n""#),
            vec![(TokenKind::InterpolatedStringLiteral, r#"$@"{a}\n""#)]
        );
        assert_eq!(
            lex(r#"$"""{a}""""#),
            vec![(TokenKind::InterpolatedStringLiteral, r#"$"""{a}""""#)]
        );
    }

    #[test]
    fn char_literals() {
        assert_eq!(lex("'a'"), vec![(TokenKind::CharLiteral, "'a'")]);
        assert_eq!(lex(r"'\''"), vec![(TokenKind::CharLiteral, r"'\''")]);
        assert_eq!(
            lex(r"'\u0041'"),
            vec![(TokenKind::CharLiteral, r"'\u0041'")]
        );
    }

    #[test]
    fn unterminated_literals_stop_at_the_line_break() {
        let tokens: Vec<_> = Lexer::new("\"abc\nx").collect();
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].text, "\"abc");
        assert!(!tokens[0].is_terminated());
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
    }

    #[test]
    fn block_comment_is_not_greedy() {
        let mut lexer = Lexer::new("/* a */ x /* b */").enable_comment_token();
        assert_eq!(
            lexer.by_ref().map(|token| token.text).collect::<Vec<_>>(),
            vec!["/* a */", "x", "/* b */"]
        );
    }

    #[test]
    fn doc_comments_win_over_plain_comments() {
        assert_eq!(
            Lexer::new("/// doc")
                .enable_comment_token()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![TokenKind::LineDocComment]
        );
        assert_eq!(
            Lexer::new("/** doc */")
                .enable_comment_token()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![TokenKind::BlockDocComment]
        );
        assert_eq!(
            Lexer::new("/**/")
                .enable_comment_token()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![TokenKind::BlockComment]
        );
    }

    #[test]
    fn line_doc_comment_does_not_swallow_the_line_break() {
        let kinds = Lexer::new("/// doc\nx")
            .enable_comment_token()
            .enable_line_feed_token()
            .map(|token| token.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                TokenKind::LineDocComment,
                TokenKind::LineFeed,
                TokenKind::Identifier
            ]
        );
    }

    #[test]
    fn doc_comments_are_tokens_by_default_but_plain_comments_are_not() {
        assert_eq!(
            kinds("/// doc\nclass A { }"),
            vec![
                TokenKind::LineDocComment,
                TokenKind::Class,
                TokenKind::Identifier,
                TokenKind::BraceLeft,
                TokenKind::BraceRight,
            ]
        );

        let mut lexer = Lexer::new("// c\nclass A { }");
        assert!(lexer.by_ref().all(|token| !token.kind.is_comment()));
        assert_eq!(lexer.comments.len(), 1);
    }

    #[test]
    fn skipped_comments_are_recorded_once() {
        let mut lexer = Lexer::new("// c\nx");
        lexer.current();
        lexer.current();
        let anchor = lexer.cast_anchor();
        lexer.next();
        lexer.back_to_anchor(anchor);
        lexer.next();

        assert_eq!(lexer.comments.len(), 1);
        assert_eq!(lexer.comments[0].text, "// c");
    }

    #[test]
    fn unexpected_character() {
        assert_eq!(
            lex("a \\ b"),
            vec![
                (TokenKind::Identifier, "a"),
                (TokenKind::UnexpectedCharacter, "\\"),
                (TokenKind::Identifier, "b"),
            ]
        );
    }

    #[test]
    fn contextual_keywords_can_be_identifiers() {
        assert_eq!(lex("var"), vec![(TokenKind::Var, "var")]);
        assert!(TokenKind::Var.can_be_identifier());
        assert!(TokenKind::Value.can_be_identifier());
        assert!(!TokenKind::Class.can_be_identifier());
        assert!(TokenKind::Class.is_reserved_keyword());
        assert!(!TokenKind::Var.is_reserved_keyword());
        assert!(!TokenKind::Identifier.is_contextual_keyword());
    }

    /// `is_reserved_keyword` / `is_contextual_keyword` are declaration-order ranges,
    /// so guard the boundaries against reordering.
    #[test]
    fn keyword_classification_matches_the_tokenizer_table() {
        for tokenizer in super::TOKENIZERS {
            let super::Tokenizer::Keyword(kind, text) = tokenizer else {
                continue;
            };

            let is_word = text.chars().all(|char| char.is_ascii_alphabetic());
            assert_eq!(
                kind.is_keyword(),
                is_word,
                "{text:?} ({kind:?}) is classified wrong"
            );
        }

        for kind in [
            TokenKind::Identifier,
            TokenKind::IntegerLiteral,
            TokenKind::LineComment,
            TokenKind::Whitespace,
            TokenKind::UnexpectedCharacter,
            TokenKind::None,
        ] {
            assert!(!kind.is_keyword(), "{kind:?} is not a keyword");
        }
    }

    #[test]
    fn fixed_text() {
        assert_eq!(TokenKind::Str.fixed_text(), Some("string"));
        assert_eq!(TokenKind::FatArrow.fixed_text(), Some("=>"));
        assert_eq!(TokenKind::Identifier.fixed_text(), None);
    }

    #[test]
    fn spans_cover_the_whole_source() {
        let source = "public void F() { int x = 1; }";
        let mut end = 0;
        for token in Lexer::new(source) {
            assert!(token.span.start >= end);
            assert_eq!(&source[token.span.clone()], token.text);
            end = token.span.end;
        }
        assert_eq!(end, source.len());
    }
}
