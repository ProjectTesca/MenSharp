//! Parser for MenSharp, a UdonSharp compatible language that stays closer to C#.
//!
//! ```no_run
//! use men_sharp_parser::MenSharpAST;
//!
//! let ast = MenSharpAST::parse("class Foo { void Bar() { } }");
//! assert!(ast.errors().is_empty());
//! ```

use std::{mem::transmute, sync::Arc};

use bumpalo::Bump;

use crate::{
    ast::CompilationUnit,
    directive::{Directive, parse_directive},
    error::ParseError,
    lexer::{Lexer, Token},
    parser::parse_compilation_unit,
};

pub mod ast;
pub mod directive;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod preprocess;

/// A parsed source file, together with the arena its tree lives in.
///
/// # Why this type exists
///
/// The tree is full of `&'input str` and `&'allocator T` borrows, which is what makes
/// parsing cheap -- no string copies, no per-node heap allocation. It also means the tree
/// cannot be returned from the function that owns the source text and the arena.
///
/// So the tree is *frozen*: the source and the arena are moved into `Arc`s here, and the
/// tree is stored with its lifetimes erased. [`MenSharpAST::ast`] hands it back re-bound
/// to `&self`, which is the real lifetime, so no borrow can outlive the arena.
///
/// Freezing is also what makes the tree `Send + Sync`, so a later compiler stage can run
/// several files (or several passes over one file) across threads while sharing it by
/// `Arc`. Cloning is a refcount bump -- clones share one arena and one copy of the source.
#[derive(Debug, Clone)]
pub struct MenSharpAST {
    /// Lifetimes erased. Only ever handed out through [`MenSharpAST::ast`], which re-binds
    /// them to the borrow of `self`.
    ast: &'static CompilationUnit<'static, 'static>,
    /// Keeps the text alive that every identifier and literal in the tree points into.
    /// `Arc<str>` rather than `String` on purpose: cloning must not move the bytes.
    source: Arc<str>,
    errors: Arc<[ParseError]>,
    /// Preprocessor directives, in source order. Lifetimes erased like `ast`.
    directives: Arc<[Directive<'static>]>,
    /// Comments that are not documentation, in source order. Lifetimes erased like `ast`.
    comments: Arc<[Token<'static>]>,
    /// Keeps every node in the tree alive. Never handed out, and never allocated from
    /// again after `parse` returns -- see the `Sync` justification below.
    allocator: Arc<Bump>,
}

// SAFETY:
//
// `Send`: everything reachable from `ast` is a shared reference into the arena, a `&str`
// into `source`, or a plain `Range<usize>` / enum. None of it has thread affinity, and the
// arena and the text are kept alive by the `Arc`s in this struct, which travel with it.
//
// `Sync`: `Bump` is deliberately not `Sync`, because `Bump::alloc` takes `&self` and moves
// its chunk pointer. That is exactly the operation this type never performs after
// construction: `allocator` is private, no method returns a `&Bump` or a `&mut Bump`, and
// parsing happens once, inside `parse`, before the value exists. What remains reachable is
// immutable, so concurrent `&MenSharpAST` readers cannot race.
//
// The `'static` in the field type is not a claim that the data is immortal; it is erased
// state. `ast()` is the only reader and it shortens the lifetime back to `&'this self`, so
// a caller can never hold a node reference after the arena has been dropped.
unsafe impl Send for MenSharpAST {}
unsafe impl Sync for MenSharpAST {}

impl MenSharpAST {
    /// Parses a source file. Always succeeds: syntax errors are collected rather than
    /// thrown, and the tree keeps a hole where each broken construct was.
    pub fn parse(source: impl Into<Arc<str>>) -> Self {
        Self::parse_with_defines(source, &[])
    }

    /// [`MenSharpAST::parse`] with conditional compilation: the arms of
    /// `#if` that `defines` leaves inactive are blanked before lexing (see
    /// [`crate::preprocess`]), so [`MenSharpAST::source`] is the text as
    /// compiled — same length and line structure as the original.
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "`Bump` is not `Sync`, which is the whole point of the `Sync` justification \
                  above: the arena is sealed after this function returns, so sharing it is safe"
    )]
    pub fn parse_with_defines(source: impl Into<Arc<str>>, defines: &[&str]) -> Self {
        let source: Arc<str> = source.into();
        let source: Arc<str> = match preprocess::preprocess(&source, defines) {
            std::borrow::Cow::Borrowed(_) => source,
            std::borrow::Cow::Owned(blanked) => Arc::from(blanked),
        };
        let allocator = Arc::new(Bump::new());
        let mut errors = std::vec::Vec::new();

        // Both borrows point at `Arc` contents, which do not move when the `Arc`s below do.
        let mut lexer = Lexer::new(&source);
        let ast = parse_compilation_unit(&mut lexer, &mut errors, allocator.as_ref());

        let directives: Arc<[Directive]> = lexer
            .directives
            .iter()
            .map(parse_directive)
            .collect::<std::vec::Vec<_>>()
            .into();
        let comments: Arc<[Token]> = lexer.comments.as_slice().into();

        // SAFETY: erases lifetimes that the accessors immediately re-bind to `&self`. The
        // arena and the source text outlive these references because this struct owns them.
        let ast: &'static CompilationUnit<'static, 'static> = unsafe { transmute(ast) };
        let directives: Arc<[Directive<'static>]> = unsafe { transmute(directives) };
        let comments: Arc<[Token<'static>]> = unsafe { transmute(comments) };

        Self {
            ast,
            source,
            errors: errors.into(),
            directives,
            comments,
            allocator,
        }
    }

    /// The syntax tree, borrowed for as long as this value is.
    pub fn ast<'this>(&'this self) -> &'this CompilationUnit<'this, 'this> {
        self.ast
    }

    /// Syntax errors, in source order.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Preprocessor directives, in source order.
    ///
    /// They are trivia to the grammar, so they are reported here rather than placed in the
    /// tree. Nothing has been evaluated: every `#if` arm was parsed. See [`directive`].
    pub fn directives<'this>(&'this self) -> &'this [Directive<'this>] {
        &self.directives
    }

    /// Non-documentation comments, in source order. Doc comments are on the declarations
    /// they belong to instead.
    pub fn comments<'this>(&'this self) -> &'this [Token<'this>] {
        &self.comments
    }

    /// The source text the tree's `&str`s point into. Use it to turn a span into text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The text a node's span covers.
    pub fn text(&self, span: &std::ops::Range<usize>) -> &str {
        &self.source[span.clone()]
    }

    /// Bytes the arena has reserved for the tree.
    pub fn allocated_bytes(&self) -> usize {
        self.allocator.allocated_bytes()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        MenSharpAST,
        ast::{
            BinaryOperator, ClassDeclaration, ClassKind, Expression, FunctionBody,
            InitializerValue, NamespaceMember, Statement, TypeDeclaration, TypeMember,
        },
    };

    static UDON_SOURCE: &str = r#"
using UdonSharp;
using UnityEngine;
using Sync = UdonSharp.BehaviourSyncMode;

namespace Game.Counters
{
    /// <summary>Counts interactions.</summary>
    [UdonBehaviourSyncMode(Sync.Manual)]
    public class Counter : UdonSharpBehaviour
    {
        [SerializeField] private Text _label = null;
        [UdonSynced] private int _count, _peak = 0;

        private const float Interval = 0.5f;

        public int Count
        {
            get => _count;
            private set
            {
                _count = value;
                _peak = value > _peak ? value : _peak;
            }
        }

        public string Label { get; set; } = "counter";

        public Counter() : this(0) { }

        public Counter(int initial)
        {
            _count = initial;
        }

        public override void Interact()
        {
            _count++;

            var flags = (1 << 3) | 0xFF;
            var nested = new List<List<int>>();
            var shifted = flags >> 2;
            shifted >>= 1;

            for (int i = 0; i < 10; i++)
            {
                if (i % 2 == 0 && !string.IsNullOrEmpty(Label))
                {
                    nested.Add(new List<int> { i, i * 2 });
                }
                else if (i is > 5 and < 9)
                {
                    continue;
                }
            }

            foreach (var item in nested)
            {
                Debug.Log($"item = {item.Count}");
            }

            switch (_count)
            {
                case 0:
                case 1:
                    Reset();
                    break;
                case int n when n > 100:
                    Debug.Log("big");
                    break;
                default:
                    break;
            }

            object boxed = _count;
            if (boxed is int value && value > 0)
            {
                Debug.Log(value.ToString());
            }

            var text = _count switch
            {
                0 => "zero",
                _ => "many",
            };

            try
            {
                Apply(x => x + 1, out int written);
            }
            catch (System.Exception e) when (e != null)
            {
                Debug.LogError(e);
            }
            finally
            {
                Reset();
            }
        }

        private void Apply(System.Func<int, int> f, out int written)
        {
            written = f(_count);
        }

        private void Reset() => _count = 0;

        public int this[int index] => index + _count;
    }

    public enum Mode : byte
    {
        Idle = 0,
        Running,
    }

    public interface ICounter<T> where T : struct
    {
        T Value { get; }
        void Add(T amount);
    }
}
"#;

    fn class_of(ast: &MenSharpAST) -> &ClassDeclaration<'_, '_> {
        let NamespaceMember::Namespace(namespace) = &ast.ast().members[0] else {
            panic!("expected a namespace");
        };
        let NamespaceMember::Type(TypeDeclaration::Class(class)) = &namespace.members[0] else {
            panic!("expected a class");
        };
        class
    }

    #[test]
    fn parses_a_realistic_file_without_errors() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        assert_eq!(ast.errors(), &[], "unexpected parse errors");
    }

    #[test]
    fn using_directives_and_aliases() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let usings = ast.ast().usings;

        assert_eq!(usings.len(), 3);
        assert!(usings[0].alias.is_none());
        assert_eq!(usings[2].alias.as_ref().unwrap().value, "Sync");
    }

    #[test]
    fn members_are_classified() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let class = class_of(&ast);

        assert_eq!(class.kind.value, ClassKind::Class);
        assert_eq!(class.name.as_ref().unwrap().value, "Counter");
        assert_eq!(class.base_types.as_ref().unwrap().types.len(), 1);

        let members = class.members.unwrap();
        let counts = members.iter().fold([0usize; 5], |mut counts, member| {
            match member {
                TypeMember::Field(_) => counts[0] += 1,
                TypeMember::Property(_) => counts[1] += 1,
                TypeMember::Constructor(_) => counts[2] += 1,
                TypeMember::Method(_) => counts[3] += 1,
                TypeMember::Indexer(_) => counts[4] += 1,
                other => panic!("unexpected member: {other:?}"),
            }
            counts
        });

        assert_eq!(counts, [3, 2, 2, 3, 1]);
    }

    #[test]
    fn a_field_can_declare_several_names() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let TypeMember::Field(field) = &class_of(&ast).members.unwrap()[1] else {
            panic!("expected a field");
        };

        assert_eq!(field.declarators.len(), 2);
        assert_eq!(field.declarators[0].name.value, "_count");
        assert_eq!(field.declarators[1].name.value, "_peak");
        assert_eq!(field.attributes.len(), 1);
    }

    #[test]
    fn doc_comments_attach_to_the_declaration() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let class = class_of(&ast);

        assert_eq!(class.documents.documents.len(), 1);
        assert!(class.documents.documents[0].value.contains("Counts"));
    }

    #[test]
    fn property_accessors_and_initializer() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let TypeMember::Property(property) = &class_of(&ast).members.unwrap()[4] else {
            panic!("expected a property");
        };

        assert_eq!(property.name.value, "Label");
        let FunctionBody::Accessors(accessors) = &property.body else {
            panic!("expected accessors");
        };
        assert_eq!(accessors.accessors.len(), 2);
        assert!(property.initializer.is_some());
    }

    #[test]
    fn expression_bodied_method() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let members = class_of(&ast).members.unwrap();
        let TypeMember::Method(method) = &members[members.len() - 2] else {
            panic!("expected a method");
        };

        assert_eq!(method.name.value, "Reset");
        assert!(matches!(method.body, FunctionBody::Expression { .. }));
    }

    #[test]
    fn interface_and_enum_are_parsed() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let NamespaceMember::Namespace(namespace) = &ast.ast().members[0] else {
            panic!("expected a namespace");
        };

        let NamespaceMember::Type(TypeDeclaration::Enum(mode)) = &namespace.members[1] else {
            panic!("expected an enum");
        };
        assert_eq!(mode.members.unwrap().len(), 2);
        assert!(mode.underlying_type.is_some());

        let NamespaceMember::Type(TypeDeclaration::Class(interface)) = &namespace.members[2] else {
            panic!("expected an interface");
        };
        assert_eq!(interface.kind.value, ClassKind::Interface);
        assert_eq!(interface.generics.as_ref().unwrap().parameters.len(), 1);
        assert_eq!(interface.constraints.len(), 1);
    }

    // ---------- disambiguation ----------

    fn statements_of(source: &str) -> MenSharpAST {
        MenSharpAST::parse(format!("class T {{ void M() {{ {source} }} }}"))
    }

    fn first_statement(ast: &MenSharpAST) -> &Statement<'_, '_> {
        &block_of(ast).statements[0]
    }

    fn block_of(ast: &MenSharpAST) -> &crate::ast::Block<'_, '_> {
        let NamespaceMember::Type(TypeDeclaration::Class(class)) = &ast.ast().members[0] else {
            panic!("expected a class");
        };
        let TypeMember::Method(method) = &class.members.unwrap()[0] else {
            panic!("expected a method");
        };
        let FunctionBody::Block(block) = &method.body else {
            panic!("expected a block body");
        };
        block
    }

    fn initializer_of<'a>(statement: &'a Statement<'_, '_>) -> &'a Expression<'a, 'a> {
        let Statement::LocalVariable(declaration) = statement else {
            panic!("expected a declaration");
        };
        let Some(InitializerValue::Expression(expression)) =
            &declaration.declarators[0].initializer
        else {
            panic!("expected an initializer expression");
        };
        expression
    }

    #[test]
    fn nested_generics_are_not_a_right_shift() {
        let ast = statements_of("var x = new Dictionary<string, List<int>>();");
        assert_eq!(ast.errors(), &[]);
        assert!(matches!(first_statement(&ast), Statement::LocalVariable(_)));
    }

    #[test]
    fn right_shift_still_works() {
        for source in [
            "var a = b >> 2;",
            "var a = b >>> 2;",
            "a >>= 2;",
            "a >>>= 2;",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }
    }

    #[test]
    fn a_comparison_chain_is_not_a_generic_name() {
        let ast = statements_of("bool r = a < b && c > d;");
        assert_eq!(ast.errors(), &[]);

        let Expression::Binary(binary) = initializer_of(first_statement(&ast)) else {
            panic!("expected a binary expression");
        };
        assert_eq!(binary.operator.value, BinaryOperator::LogicalAnd);
    }

    #[test]
    fn a_cast_is_told_apart_from_a_parenthesised_expression() {
        // `(int)` names a type, so `-` starts the operand
        let ast = statements_of("var a = (int)-b;");
        assert_eq!(ast.errors(), &[]);
        assert!(matches!(
            initializer_of(first_statement(&ast)),
            Expression::Cast(_)
        ));

        // `(x)` is just a value, so `-` is a subtraction
        let ast = statements_of("var a = (x)-b;");
        assert_eq!(ast.errors(), &[]);
        assert!(matches!(
            initializer_of(first_statement(&ast)),
            Expression::Binary(_)
        ));
    }

    #[test]
    fn a_ternary_is_not_a_nullable_declaration() {
        let ast = statements_of("var r = flag ? a : b;");
        assert_eq!(ast.errors(), &[]);
        assert!(matches!(
            initializer_of(first_statement(&ast)),
            Expression::Conditional(_)
        ));
    }

    #[test]
    fn a_nullable_declaration_is_still_a_declaration() {
        let ast = statements_of("Foo? handle = null;");
        assert_eq!(ast.errors(), &[]);

        let Statement::LocalVariable(declaration) = first_statement(&ast) else {
            panic!("expected a declaration");
        };
        assert_eq!(declaration.variable_type.suffixes.len(), 1);
    }

    #[test]
    fn lambdas_in_argument_position() {
        for source in [
            "Run(x => x + 1);",
            "Run((x, y) => x + y);",
            "Run((int x) => { return x; });",
            "Run(async x => await x);",
            "Run(() => 0);",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }
    }

    #[test]
    fn anonymous_methods() {
        for source in [
            "Run(delegate { });",
            "Run(delegate(int x) { return x; });",
            "Run(async delegate(int x) { await F(x); });",
            "System.Action a = delegate { };",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }

        // `delegate { }` leaves the parameter list out entirely, which is not the same
        // thing as writing `delegate() { }`
        let ast = statements_of("System.Action a = delegate { };");
        let Expression::AnonymousMethod(anonymous) = initializer_of(first_statement(&ast)) else {
            panic!("expected an anonymous method");
        };
        assert!(anonymous.parameters.is_none());

        let ast = statements_of("System.Action<int> a = delegate(int x) { };");
        let Expression::AnonymousMethod(anonymous) = initializer_of(first_statement(&ast)) else {
            panic!("expected an anonymous method");
        };
        assert_eq!(anonymous.parameters.as_ref().unwrap().parameters.len(), 1);
    }

    #[test]
    fn a_delegate_declaration_is_not_an_anonymous_method() {
        let ast = MenSharpAST::parse("public delegate int Op(int a);");
        assert_eq!(ast.errors(), &[]);

        let NamespaceMember::Type(TypeDeclaration::Delegate(declaration)) = &ast.ast().members[0]
        else {
            panic!("expected a delegate declaration");
        };
        assert_eq!(declaration.name.as_ref().unwrap().value, "Op");
    }

    /// `Type Name (...)` is the shape of both a local function and a call, and only the
    /// body that follows tells them apart.
    #[test]
    fn a_call_is_not_a_local_function() {
        for source in ["await F(x);", "await this.F(x);", "await F<int>(x);"] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
            assert!(
                !matches!(first_statement(&ast), Statement::LocalFunction(_)),
                "{source:?} should not be a local function"
            );
        }

        for source in [
            "int F(int x) { return x; }",
            "int F(int x) => x;",
            "T F<T>(T x) where T : struct { return x; }",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
            assert!(
                matches!(first_statement(&ast), Statement::LocalFunction(_)),
                "{source:?} should be a local function"
            );
        }
    }

    /// `Foo * bar;` is a multiplication outside `unsafe` and a pointer declaration inside,
    /// which is exactly where C# draws the line. A predefined type settles it either way,
    /// because `int` cannot be multiplied.
    #[test]
    fn a_pointer_declaration_needs_an_unsafe_scope() {
        let ast = statements_of("a * b;");
        assert_eq!(ast.errors(), &[]);
        assert!(matches!(first_statement(&ast), Statement::Expression(_)));

        let ast = statements_of("int* p;");
        assert_eq!(ast.errors(), &[]);
        let Statement::LocalVariable(declaration) = first_statement(&ast) else {
            panic!("expected a declaration");
        };
        assert!(matches!(
            declaration.variable_type.suffixes[0],
            crate::ast::TypeSuffix::Pointer { .. }
        ));

        let ast = MenSharpAST::parse("class T { unsafe void M() { Foo* p; } }");
        assert_eq!(ast.errors(), &[]);
        let Statement::LocalVariable(declaration) = first_statement(&ast) else {
            panic!("expected a pointer declaration inside unsafe");
        };
        assert!(matches!(
            declaration.variable_type.suffixes[0],
            crate::ast::TypeSuffix::Pointer { .. }
        ));
    }

    // ---------- interpolated strings ----------

    fn interpolation_of(source: &str) -> MenSharpAST {
        MenSharpAST::parse(format!("class T {{ void M() {{ var s = {source}; }} }}"))
    }

    fn parts_of(ast: &MenSharpAST) -> &crate::ast::InterpolatedString<'_, '_> {
        let Expression::Primary(primary) = initializer_of(first_statement(ast)) else {
            panic!("expected a primary expression");
        };
        let crate::ast::PrimaryLeft::Literal(crate::ast::LiteralExpression::InterpolatedString(
            interpolated,
        )) = &primary.left
        else {
            panic!("expected an interpolated string");
        };
        interpolated
    }

    #[test]
    fn interpolation_holes_become_expressions() {
        use crate::ast::InterpolationPart;

        let ast = interpolation_of(r#"$"count = {_count + 1} items""#);
        assert_eq!(ast.errors(), &[]);

        let parts = parts_of(&ast).parts;
        assert_eq!(parts.len(), 3);

        let InterpolationPart::Text(text) = &parts[0] else {
            panic!("expected leading text");
        };
        assert_eq!(text.value, "count = ");

        let InterpolationPart::Hole(hole) = &parts[1] else {
            panic!("expected a hole");
        };
        // the hole really is a tree, not a string
        assert!(matches!(
            hole.expression.as_ref().unwrap(),
            Expression::Binary(_)
        ));

        let InterpolationPart::Text(text) = &parts[2] else {
            panic!("expected trailing text");
        };
        assert_eq!(text.value, " items");
    }

    #[test]
    fn alignment_and_format_are_separated() {
        use crate::ast::InterpolationPart;

        let ast = interpolation_of(r#"$"{value,-5:F2}""#);
        assert_eq!(ast.errors(), &[]);

        let InterpolationPart::Hole(hole) = &parts_of(&ast).parts[0] else {
            panic!("expected a hole");
        };
        assert!(hole.expression.is_ok());
        assert!(
            hole.alignment.is_some(),
            "`-5` should parse as an expression"
        );
        assert_eq!(hole.format.as_ref().unwrap().value, "F2");
    }

    /// The separators are the first `,` and `:` outside brackets, so a comma inside a
    /// call is an argument and a `::` is a qualifier.
    #[test]
    fn separators_respect_nesting() {
        use crate::ast::InterpolationPart;

        let ast = interpolation_of(r#"$"{Format(a, b)}""#);
        assert_eq!(ast.errors(), &[]);
        let InterpolationPart::Hole(hole) = &parts_of(&ast).parts[0] else {
            panic!("expected a hole");
        };
        assert!(hole.comma.is_none(), "the comma belongs to the call");

        let ast = interpolation_of(r#"$"{global::System.Int32.MaxValue}""#);
        assert_eq!(ast.errors(), &[]);
        let InterpolationPart::Hole(hole) = &parts_of(&ast).parts[0] else {
            panic!("expected a hole");
        };
        assert!(hole.colon.is_none(), "`::` is not a format separator");

        let ast = interpolation_of(r#"$"{new Foo { A = 1 }}""#);
        assert_eq!(ast.errors(), &[]);
        assert_eq!(parts_of(&ast).parts.len(), 1);
    }

    /// C# ends the expression at the first bare `:`, which is why a conditional in a hole
    /// has to be parenthesised. Matching that keeps this parser from accepting code the
    /// C# language service rejects.
    #[test]
    fn a_conditional_in_a_hole_needs_parentheses() {
        let ast = interpolation_of(r#"$"{flag ? a : b}""#);
        assert!(
            !ast.errors().is_empty(),
            "a bare conditional should be rejected, as in C#"
        );

        let ast = interpolation_of(r#"$"{(flag ? a : b)}""#);
        assert_eq!(ast.errors(), &[]);
    }

    #[test]
    fn interpolation_forms() {
        for source in [
            r#"$"plain""#,
            r#"$"{{ escaped braces }}""#,
            r#"$"{a}{b}""#,
            r#"$"{f("}")}""#,
            r#"$@"{a}\not an escape""#,
            r#"@$"{a}""#,
            r#"$"""{a} raw""""#,
            r#"$"{xs.Select(y => y * 2).Count()}""#,
            r#"$"{await F()}""#,
            r#"$"{x = 5}""#,
            r#"$"{arr[i, j]}""#,
            r#"$"{d:yyyy-MM-dd}""#,
        ] {
            let ast = interpolation_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source}");
        }
    }

    #[test]
    fn hole_spans_index_back_into_the_file() {
        use crate::ast::InterpolationPart;

        let source = r#"class T { void M() { var s = $"n = {count}"; } }"#;
        let ast = MenSharpAST::parse(source);
        assert_eq!(ast.errors(), &[]);

        let InterpolationPart::Hole(hole) = &parts_of(&ast).parts[1] else {
            panic!("expected a hole");
        };
        let expression = hole.expression.as_ref().unwrap();

        // the sub lexer reports positions in the whole file, not in the hole
        assert_eq!(ast.text(&expression.span()), "count");
    }

    #[test]
    fn a_broken_hole_is_reported_inside_the_literal() {
        let source = r#"class T { void M() { var s = $"{a +}"; } }"#;
        let ast = MenSharpAST::parse(source);

        assert!(!ast.errors().is_empty());
        for error in ast.errors() {
            assert!(error.span.end <= source.len());
        }
    }

    #[test]
    fn deconstruction_ref_and_collection_expressions() {
        for source in [
            "var (a, b) = t;",
            "var (a, (b, c)) = t;",
            "var (a, _) = t;",
            "(int a, string b) = t;",
            "(a, b) = t;",
            "ref int y = ref x;",
            "int[] a = [1, 2, 3];",
            "int[] a = [.. first, 4, .. second];",
            "List<int> a = [];",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }

        // `var (a, b) = t` is an assignment whose left side declares the variables
        let ast = statements_of("var (a, b) = t;");
        let Statement::Expression(statement) = first_statement(&ast) else {
            panic!("expected an expression statement");
        };
        let Expression::Assignment(assignment) = &statement.expression else {
            panic!("expected an assignment");
        };
        let Expression::Declaration(declaration) = &assignment.target else {
            panic!("expected a declaration on the left");
        };
        assert!(matches!(
            declaration.designation,
            crate::ast::VariableDesignation::Parenthesized { .. }
        ));
    }

    /// A call must not read as a declaration just because `f (x)` has the same shape.
    #[test]
    fn a_call_is_not_a_declaration_expression() {
        for source in ["f(x);", "F(a, b);", "var r = f(x);"] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }

        let ast = statements_of("f(x);");
        let Statement::Expression(statement) = first_statement(&ast) else {
            panic!("expected an expression statement");
        };
        assert!(matches!(statement.expression, Expression::Primary(_)));
    }

    #[test]
    fn extern_alias() {
        let ast = MenSharpAST::parse("extern alias Legacy;\nusing System;\nclass A { }");
        assert_eq!(ast.errors(), &[]);
        assert_eq!(ast.ast().extern_aliases.len(), 1);
        assert_eq!(
            ast.ast().extern_aliases[0].name.as_ref().unwrap().value,
            "Legacy"
        );

        // `extern` on its own is still a member modifier
        let ast = MenSharpAST::parse("class A { extern void M(); }");
        assert_eq!(ast.errors(), &[]);
        assert!(ast.ast().extern_aliases.is_empty());
    }

    #[test]
    fn positional_and_list_patterns() {
        for source in [
            "if (p is (0, 0)) { }",
            "if (p is Point(0, var y)) { }",
            "if (p is Point(X: 0, Y: 0)) { }",
            "if (p is Point(0, 0) { Z: 1 } q) { }",
            "if (a is [1, 2, 3]) { }",
            "if (a is [1, ..]) { }",
            "if (a is [first, .. var rest, last]) { }",
            "if (a is [] or [_]) { }",
            "if (t is var (x, y)) { }",
            "if (t is var (x, (y, z))) { }",
            "if (t is var (x, _)) { }",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }
    }

    /// `(P)` is a parenthesised pattern; anything with a comma, a name, a property part
    /// or a binding is positional.
    #[test]
    fn a_single_element_group_stays_parenthesised() {
        fn pattern_of(source: &str) -> &'static str {
            let ast = statements_of(&format!("if (x is {source}) {{ }}"));
            assert_eq!(ast.errors(), &[], "failed on {source:?}");

            let Statement::If(statement) = first_statement(&ast) else {
                panic!("expected an if");
            };
            let Ok(Expression::Is(is)) = &statement.condition else {
                panic!("expected an is expression");
            };
            match is.pattern.as_ref().unwrap() {
                crate::ast::Pattern::Parenthesized { .. } => "parenthesized",
                crate::ast::Pattern::Positional { .. } => "positional",
                other => panic!("unexpected pattern: {other:?}"),
            }
        }

        assert_eq!(pattern_of("(0)"), "parenthesized");
        assert_eq!(pattern_of("(0 or 1)"), "parenthesized");
        assert_eq!(pattern_of("(0, 1)"), "positional");
        assert_eq!(pattern_of("(X: 0)"), "positional");
        assert_eq!(pattern_of("(0) p"), "positional");
    }

    #[test]
    fn linq_query_syntax() {
        for source in [
            "var q = from x in xs select x;",
            "var q = from int x in xs select x;",
            "var q = from x in xs where x > 0 select x * 2;",
            "var q = from x in xs let y = x * 2 where y > 0 select y;",
            "var q = from x in xs orderby x.Name, x.Age descending select x;",
            "var q = from x in xs from y in ys select x + y;",
            "var q = from x in xs join y in ys on x.Id equals y.Id select y;",
            "var q = from x in xs join y in ys on x.Id equals y.Id into g select g;",
            "var q = from x in xs group x by x.Key;",
            "var q = from x in xs group x by x.Key into g select g.Count();",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }

        let ast = statements_of("var q = from x in xs where x > 0 select x;");
        let Expression::Query(query) = initializer_of(first_statement(&ast)) else {
            panic!("expected a query expression");
        };
        assert_eq!(query.from.name.value, "x");
        assert_eq!(query.body.clauses.len(), 1);
        assert!(matches!(
            query.body.select_or_group,
            Ok(crate::ast::SelectOrGroupClause::Select { .. })
        ));
    }

    /// The query keywords are contextual, so they have to stay usable as names.
    #[test]
    fn query_keywords_are_still_identifiers() {
        for source in [
            "var from = 1;",
            "from = 1;",
            "var x = select + where;",
            "join(a, b);",
            "var y = group.Count;",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
            assert!(
                !matches!(first_statement(&ast), Statement::Expression(statement)
                    if matches!(statement.expression, Expression::Query(_))),
                "{source:?} should not be a query"
            );
        }
    }

    #[test]
    fn unsafe_constructs() {
        for source in [
            "unsafe { int* p = null; }",
            "unsafe { fixed (int* p = &array[0]) { *p = 1; } }",
            "unsafe { var q = stackalloc int[10]; }",
            "unsafe { var q = stackalloc int[] { 1, 2 }; }",
            "unsafe { var q = stackalloc[] { 1, 2 }; }",
            "unsafe { var v = p->field; }",
            "unsafe { var v = &x; var w = *p; }",
            "unsafe { var n = sizeof(int*); var c = (byte*)p; }",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }

        let ast = MenSharpAST::parse(
            "public unsafe class A { private byte* _data; void M() { Foo* q = null; } }",
        );
        assert_eq!(ast.errors(), &[]);
    }

    #[test]
    fn array_creation_forms() {
        for source in [
            "var a = new int[10];",
            "var a = new int[] { 1, 2 };",
            "var a = new[] { 1, 2 };",
            "var a = new int[2, 3];",
            "var a = new int[n][];",
        ] {
            let ast = statements_of(source);
            assert_eq!(ast.errors(), &[], "failed on {source:?}");
        }
    }

    /// One line per C# construct the parser claims to support. Anything that produces an
    /// error here is a gap, so new syntax gets a row rather than its own test.
    #[test]
    fn the_supported_c_sharp_surface_parses_cleanly() {
        const CASES: &[(&str, &str)] = &[
            ("file scoped namespace", "namespace A.B;\nclass C { }"),
            ("global using", "global using System;\nclass C { }"),
            ("using static", "using static System.Math;\nclass C { }"),
            ("record", "public record Point(int X, int Y);"),
            ("record struct", "public readonly record struct P(int X);"),
            (
                "delegate",
                "public delegate int Op<T>(T a, T b) where T : struct;",
            ),
            ("nested type", "class A { private class B { int x; } }"),
            ("static constructor", "class A { static A() { } }"),
            ("destructor", "class A { ~A() { } }"),
            (
                "operator",
                "class A { public static A operator +(A a, A b) => a; \
                 public static bool operator ==(A a, A b) => true; }",
            ),
            (
                "shift operator",
                "class A { public static A operator >>(A a, int b) => a; }",
            ),
            (
                "conversion operator",
                "class A { public static implicit operator int(A a) => 0; \
                 public static explicit operator A(int i) => null; }",
            ),
            (
                "explicit interface implementation",
                "class A : IB { void IB.M() { } int IB.P { get { return 0; } } }",
            ),
            (
                "indexer",
                "class A { public int this[int i, string s] { get => 0; set { } } }",
            ),
            (
                "event",
                "class A { public event System.Action Changed; \
                 public event System.Action E { add { } remove { } } }",
            ),
            (
                "parameter modifiers",
                "class A { void M(params int[] xs, int y = 1, ref int z, out int w, in int v) { } }",
            ),
            (
                "attribute targets",
                "[assembly: Foo]\n[return: Bar]\nclass A { [field: NonSerialized] int x; }",
            ),
            (
                "generic constraints",
                "class A<T, U> where T : class?, new() where U : unmanaged { }",
            ),
            (
                "nullable and array types",
                "class A { int?[][,] x; System.Collections.Generic.List<int?> y; }",
            ),
            (
                "tuple type",
                "class A { (int a, string b) M() => (1, \"x\"); }",
            ),
            (
                "local function",
                "class A { void M() { int F(int x) => x * 2; F(1); } }",
            ),
            (
                "goto",
                "class A { void M() { switch (x) { case 1: goto case 2; case 2: goto done; } \
                 done: return; } }",
            ),
            (
                "do while and lock",
                "class A { void M() { do { } while (a < b); lock (this) { } } }",
            ),
            (
                "using statement and declaration",
                "class A { void M() { using (var s = F()) { } using var t = G(); } }",
            ),
            (
                "checked",
                "class A { void M() { checked { x++; } var y = unchecked(a + b); } }",
            ),
            (
                "yield",
                "class A { System.Collections.IEnumerable M() { yield return 1; yield break; } }",
            ),
            (
                "patterns",
                "class A { void M() { if (o is not null and (int or string)) { } \
                 if (o is Point { X: > 0, Y: var y } p) { } } }",
            ),
            (
                "switch expression",
                "class A { int M(object o) => o switch { int i when i > 0 => 1, null => 0, _ => -1 }; }",
            ),
            (
                "with expression",
                "class A { P M(P p) => p with { X = 1 }; }",
            ),
            (
                "ranges and indices",
                "class A { void M() { var a = xs[1..^2]; var b = xs[..]; var c = xs[^1]; } }",
            ),
            (
                "null conditional",
                "class A { void M() { var v = a?.b?[0]?.c!.d; a ??= b; } }",
            ),
            (
                "initializers",
                "class A { void M() { \
                 var d = new Dictionary<string, int> { [\"a\"] = 1, [\"b\"] = 2 }; \
                 var o = new Foo { A = 1, B = { C = 2 } }; \
                 var l = new List<int> { 1, 2 }; \
                 var j = new int[,] { { 1, 2 }, { 3, 4 } }; } }",
            ),
            (
                "anonymous object",
                "class A { void M() { var o = new { X = 1, Y = 2 }; } }",
            ),
            (
                "unbound generics in typeof",
                "class A { void M() { var t = typeof(List<>); var u = typeof(Dictionary<,>); \
                 var v = typeof(int[]); } }",
            ),
            (
                "nameof and default",
                "class A { void M() { var n = nameof(A); var d = default(int); var e = default; } }",
            ),
            (
                "async await",
                "class A { public async System.Threading.Tasks.Task<int> M() { return await F(); } }",
            ),
            (
                "interpolated strings",
                "class A { void M() { var s = $\"{a,5:F2} and {b}\"; var v = $@\"{a}\\n\"; \
                 var r = $\"\"\"{a}\"\"\"; var n = $\"{f(x, y)}\"; } }",
            ),
            (
                "verbatim identifier",
                "class A { int @class; void M() { @class = 1; } }",
            ),
            ("global alias", "class A { global::System.Int32 x; }"),
            (
                "chained ternary",
                "class A { void M() { var x = a ? b : c ? d : e; } }",
            ),
            (
                "cast chains",
                "class A { void M() { var x = (int)(long)(object)y; var z = (List<int>)w; } }",
            ),
            (
                "deconstruction",
                "class A { void M() { (a, b) = (1, 2); } }",
            ),
            (
                "multi dimensional access",
                "class A { void M() { m[1, 2] = m[3, 4]; } }",
            ),
            (
                "named and out arguments",
                "class A { void M() { F(x: 1, y: 2); G(out var r, ref s); } }",
            ),
            (
                "nested lambdas",
                "class A { void M() { F(x => y => x + y); F(() => { return 1; }); } }",
            ),
            (
                "comments",
                "class A { /* c */ void M() { // line\n int x = 1; /** doc */ } }",
            ),
            (
                "preprocessor directives",
                "#define FOO\n#if FOO\nclass A { }\n#endif",
            ),
            ("extern alias", "extern alias Legacy;\nclass A { }"),
            (
                "unsafe members",
                "unsafe class A { byte* _p; void M() { fixed (int* q = &a[0]) { *q = 1; } } }",
            ),
            (
                "stackalloc",
                "class A { unsafe void M() { var s = stackalloc int[8]; } }",
            ),
            (
                "pointer member access",
                "class A { unsafe void M() { var v = p->f; var w = (*p).f; } }",
            ),
            (
                "linq query",
                "class A { void M() { var q = from x in xs where x > 0 orderby x descending \
                 select x; } }",
            ),
            (
                "linq join and group",
                "class A { void M() { var q = from x in xs join y in ys on x.K equals y.K into g \
                 group g by g.Key into h select h; } }",
            ),
            (
                "positional and list patterns",
                "class A { void M() { if (p is (0, var y) and not [1, ..]) { } } }",
            ),
            (
                "deconstruction",
                "class A { void M() { var (a, b) = t; (int c, string d) = u; } }",
            ),
            ("ref locals", "class A { void M() { ref int y = ref x; } }"),
            (
                "collection expressions",
                "class A { void M() { int[] a = [1, .. rest, 2]; } }",
            ),
            (
                "anonymous method",
                "class A { void M() { Run(delegate(int x) { return x; }); } }",
            ),
        ];

        let mut failures = std::vec::Vec::new();
        for (name, source) in CASES {
            let ast = MenSharpAST::parse(*source);
            if !ast.errors().is_empty() {
                failures.push(format!("{name}: {:?}", ast.errors()));
            }
        }

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // ---------- error recovery ----------

    #[test]
    fn a_missing_semicolon_does_not_stop_the_parse() {
        let ast = MenSharpAST::parse("class T { void M() { var a = 1 var b = 2; } }");

        assert!(!ast.errors().is_empty());
        assert_eq!(
            block_of(&ast).statements.len(),
            2,
            "both declarations should survive"
        );
    }

    #[test]
    fn a_broken_member_leaves_a_hole_and_the_rest_parses() {
        let ast = MenSharpAST::parse("class T { int ; void M() { } }");

        assert!(!ast.errors().is_empty());
        let NamespaceMember::Type(TypeDeclaration::Class(class)) = &ast.ast().members[0] else {
            panic!("expected a class");
        };
        assert!(
            class.members.unwrap().iter().any(
                |member| matches!(member, TypeMember::Method(method) if method.name.value == "M")
            ),
            "the method after the broken member should still be found"
        );
    }

    #[test]
    fn errors_carry_a_span_inside_the_source() {
        let source = "class T { void M() { var a = ; } }";
        let ast = MenSharpAST::parse(source);

        assert!(!ast.errors().is_empty());
        for error in ast.errors() {
            assert!(error.span.start <= error.span.end);
            assert!(error.span.end <= source.len());
        }
    }

    #[test]
    fn an_unclosed_brace_terminates_instead_of_looping() {
        let ast = MenSharpAST::parse("class T { void M() { if (a) { ");
        assert!(!ast.errors().is_empty());
    }

    #[test]
    fn garbage_input_terminates() {
        let ast = MenSharpAST::parse("} ) ] ` \\ ~ #");
        assert!(!ast.errors().is_empty());
    }

    #[test]
    fn an_empty_file_is_an_empty_tree() {
        let ast = MenSharpAST::parse("");
        assert_eq!(ast.errors(), &[]);
        assert!(ast.ast().members.is_empty());
    }

    #[test]
    fn directives_do_not_disturb_the_parse() {
        let source = "#if UNITY_EDITOR\nusing UnityEditor;\n#endif\n\
                      class A {\n#region Fields\n    int x; // note\n#endregion\n}";
        // `#if` is evaluated: without the define the `using` is gone
        let ast = MenSharpAST::parse(source);
        assert_eq!(ast.errors(), &[]);
        assert_eq!(ast.ast().usings.len(), 0);
        let ast = MenSharpAST::parse_with_defines(source, &["UNITY_EDITOR"]);

        assert_eq!(ast.errors(), &[]);
        assert_eq!(ast.ast().usings.len(), 1);

        let kinds: Vec<_> = ast
            .directives()
            .iter()
            .map(|directive| directive.kind.value)
            .collect();
        assert_eq!(
            kinds,
            vec![
                crate::directive::DirectiveKind::If,
                crate::directive::DirectiveKind::Endif,
                crate::directive::DirectiveKind::Region,
                crate::directive::DirectiveKind::EndRegion,
            ]
        );

        assert_eq!(ast.comments().len(), 1);
        assert_eq!(ast.comments()[0].text, "// note");
    }

    // ---------- the freeze ----------

    #[test]
    fn the_tree_can_be_shared_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MenSharpAST>();

        let ast = Arc::new(MenSharpAST::parse(UDON_SOURCE));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let ast = Arc::clone(&ast);
                std::thread::spawn(move || {
                    let class = class_of(&ast);
                    assert_eq!(class.name.as_ref().unwrap().value, "Counter");
                    class.members.unwrap().len()
                })
            })
            .collect();

        for handle in handles {
            assert_eq!(handle.join().unwrap(), 11);
        }
    }

    #[test]
    fn a_clone_keeps_pointing_at_the_same_arena() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let clone = ast.clone();
        drop(ast);

        assert_eq!(class_of(&clone).name.as_ref().unwrap().value, "Counter");
    }

    #[test]
    fn spans_index_back_into_the_source() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        let class = class_of(&ast);
        let name = class.name.as_ref().unwrap();

        assert_eq!(ast.text(&name.span), name.value);
    }

    #[test]
    fn the_whole_tree_lives_in_one_arena() {
        let ast = MenSharpAST::parse(UDON_SOURCE);
        assert!(ast.allocated_bytes() > 0);
    }
}
