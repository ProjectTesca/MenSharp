//! Body checking: a type for every expression, and the errors on the way.

mod common;

use common::{
    MockExternal, checked, declarations, error_kinds, find, member_signature, named_source,
};
use men_sharp_semantics::types::{MemberSignature, Type, TypeTarget};
use men_sharp_semantics::{FileId, SemanticErrorKind, SymbolKind, TypeSystem, resolve_signatures};

#[test]
pub fn locals_and_var_infer_and_convert() {
    checked!(
        check,
        r#"
        public class Body
        {
            void Run()
            {
                var count = 1;
                count = 2;
                long widened = count;
                byte narrowed = 5;
                string wrong = count;
            }
        }
        "#,
    );

    // the probe: assigning the inferred int to string names both types
    assert_eq!(check.errors.len(), 1);
    assert_eq!(
        check.errors[0].kind,
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn integer_constant_expressions_narrow_like_literals() {
    checked!(
        check,
        r#"
        public class Body
        {
            void Run()
            {
                sbyte a = 3 - 5;
                byte b = 20 / 3;
                short c = -(1 << 4);
                byte d = (0xFF & 250) | 0;
                sbyte e = ~0;
                int n = 3;
                sbyte wrong = n - 5;
            }
        }
        "#,
    );

    // only the non-constant one is a mismatch
    assert_eq!(check.errors.len(), 1);
    assert_eq!(
        check.errors[0].kind,
        SemanticErrorKind::TypeMismatch {
            expected: "sbyte".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn members_inheritance_and_conditions() {
    checked!(
        check,
        r#"
        public class Base
        {
            public int health;
            public bool IsAlive() { return health > 0; }
        }
        public class Player : Base
        {
            public string name;

            void Update()
            {
                if (IsAlive() && health < 100) { health = health + 1; }
                if (health) {}
                Missing();
                string wrong = this.unknown;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert!(matches!(
        kinds[0],
        SemanticErrorKind::ConditionNotBoolean { .. }
    ));
    assert!(matches!(kinds[1], SemanticErrorKind::UnknownIdentifier));
    assert!(matches!(kinds[2], SemanticErrorKind::UnknownMember { .. }));
}

#[test]
pub fn overloads_pick_by_argument_types() {
    checked!(
        check,
        r#"
        public class Overloads
        {
            void Take(int value) {}
            void Take(string value) {}
            string Pick(long value) { return ""; }
            int Pick(int value) { return 0; }

            void Run()
            {
                Take(1);
                Take("hello");
                Take(true);
                string probe = Pick(1);
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    // Take(true) fits neither overload
    assert!(matches!(kinds[0], SemanticErrorKind::NoMatchingOverload));
    // Pick(1) prefers the exact int overload returning int, so the string
    // probe reports int
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn generic_methods_infer_or_ask_for_annotations() {
    checked!(
        check,
        r#"
        public class Generics
        {
            T Identity<T>(T value) { return value; }
            T Make<T>() { return default(T); }

            void Run()
            {
                string ok = Identity("hello");
                string probe = Identity(1);
                string explicitly = Make<string>();
                var impossible = Make();
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
    // Make() has nothing to infer T from: the annotate-it error
    assert!(matches!(
        kinds[1],
        SemanticErrorKind::CannotInferTypeArguments
    ));
}

#[test]
pub fn enums_statics_and_value_flow() {
    checked!(
        check,
        r#"
        public enum Color { Red, Green }
        public class Statics
        {
            public static int counter;
            public int instance;

            static void Tick()
            {
                counter = counter + 1;
                instance = 2;
                var color = Color.Red;
                bool same = color == Color.Green;
                string wrong = color;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(matches!(
        kinds[0],
        SemanticErrorKind::InstanceMemberInStaticContext
    ));
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "Color".to_string(),
        }
    );
}

#[test]
pub fn arrays_foreach_and_indexing() {
    checked!(
        check,
        r#"
        public class Arrays
        {
            void Run(int[] values)
            {
                var first = values[0];
                values[1] = first + 1;
                int total = 0;
                foreach (var value in values) { total += value; }
                foreach (string wrong in values) {}
                values["x"] = 1;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    // int element into a string loop variable
    assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
    // a string index into an int[] slot
    assert!(matches!(kinds[1], SemanticErrorKind::TypeMismatch { .. }));
}

#[test]
pub fn user_defined_operators_and_string_concat() {
    checked!(
        check,
        r#"
        public struct Vec
        {
            public float x;
            public static Vec operator +(Vec a, Vec b) { return a; }
        }
        public class Ops
        {
            void Run(Vec a, Vec b, int n)
            {
                var sum = a + b;
                sum.x = 1.5f;
                string label = "n = " + n;
                var bad = a * b;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert!(matches!(
        kinds[0],
        SemanticErrorKind::InvalidOperator { .. }
    ));
}

#[test]
pub fn out_var_and_is_patterns_bind_locals() {
    checked!(
        check,
        r#"
        public class Player {}
        public class Bindings
        {
            bool TryGet(out int value) { value = 1; return true; }

            void Run(object thing)
            {
                if (TryGet(out var got)) { int use = got; }
                if (thing is Player player) { Player p = player; }
                string probe = got;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn return_types_are_enforced() {
    checked!(
        check,
        r#"
        public class Returns
        {
            int Number() { return "text"; }
            void Nothing() { return 1; }
            int Missing() { return; }
            long Widened() { return 1; }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
    assert!(matches!(kinds[1], SemanticErrorKind::ReturnValueMismatch));
    assert!(matches!(kinds[2], SemanticErrorKind::ReturnValueMismatch));
}

#[test]
pub fn unsupported_constructs_get_scaffolding_errors() {
    checked!(
        check,
        r#"
        public class Scaffolding
        {
            void Run()
            {
                var q = from x in "abc" select x;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert!(
        kinds
            .iter()
            .all(|kind| matches!(kind, SemanticErrorKind::UnsupportedExpression)),
        "{kinds:?}"
    );
    assert_eq!(kinds.len(), 1);
}

// ---------------------------------------------- spec-shaped inference

#[test]
pub fn lower_bound_inference_sees_through_interfaces() {
    checked!(
        check,
        r#"
        public interface IContainer<T> { }
        public class Box : IContainer<string> { }
        public class App
        {
            T First<T>(IContainer<T> container) { return default; }

            void Run(Box box)
            {
                string ok = First(box);
                int probe = First(box);
            }
        }
        "#,
    );

    // the probe proves T was fixed to string through Box : IContainer<string>
    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "int".to_string(),
            found: "string".to_string(),
        }
    );
}

#[test]
pub fn lambdas_check_against_delegates() {
    checked!(
        check,
        r#"
        public delegate bool Filter(int value);
        public class App
        {
            int Count(Filter filter) { return 0; }

            void Run()
            {
                var a = Count(x => x > 0);
                Filter direct = x => x != 1;
                Count((int x) => true);
                Count(x => x + 1);
                Count((string s) => true);
                string probe = a;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    // `x + 1` is int, the delegate wants bool
    assert!(matches!(kinds[0], SemanticErrorKind::TypeMismatch { .. }));
    // `(string s)` disagrees with the delegate's int
    assert!(matches!(kinds[1], SemanticErrorKind::TypeMismatch { .. }));
    // the probe proves Count(...) returned int
    assert_eq!(
        *kinds[2],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn two_phase_inference_flows_through_lambda_returns() {
    checked!(
        check,
        r#"
        public delegate R Map<T, R>(T value);
        public class App
        {
            R Apply<T, R>(T value, Map<T, R> mapper) { return default; }

            void Run()
            {
                string probe = Apply(1, x => x > 0);
            }
        }
        "#,
    );

    // T = int fixes from the first argument, the lambda's body then types as
    // bool, and R = bool comes back out — visible in the probe
    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "bool".to_string(),
        }
    );
}

#[test]
pub fn inference_feeds_one_lambda_into_the_next_and_ranks_lambda_returns() {
    checked!(
        check,
        r#"
        public delegate R Map<T, R>(T value);
        public delegate R Join<A, B, R>(A a, B b);
        public class App
        {
            R Chain<T, U, R>(T value, Map<T, U> first, Join<T, U, R> second) { return default; }
            int Sum<T>(T value, Map<T, int> f) { return 0; }
            long Sum<T>(T value, Map<T, long> f) { return 0; }
            double Sum<T>(T value, Map<T, double> f) { return 0; }

            void Run()
            {
                // the second lambda's inputs are fixed by the first one's return
                int probe_chain = Chain(1, x => x > 0, (x, flag) => flag ? "y" : "n");
                // a lambda returning int fits all three; the int overload is the better one
                string probe_int = Sum(1, x => x + 1);
                string probe_double = Sum(1, x => 2.5);
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    for (kind, found) in kinds.iter().zip(["string", "int", "double"]) {
        assert_eq!(
            **kind,
            SemanticErrorKind::TypeMismatch {
                expected: if found == "string" { "int" } else { "string" }.to_string(),
                found: found.to_string(),
            }
        );
    }
}

#[test]
pub fn natural_lambda_and_best_common_types() {
    checked!(
        check,
        r#"
        public class Animal { }
        public class Dog : Animal { }
        public class App
        {
            void Run(bool flag, Animal animal, Dog dog)
            {
                var f = (int x) => x + 1;
                string probe_f = f;

                var pick = flag ? dog : animal;
                string probe_pick = pick;

                var mixed = flag ? 1 : 2.0;
                string probe_mixed = mixed;

                var xs = new[] { 1, 2L };
                string probe_xs = xs;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "System.Func<int, int>".to_string(),
        }
    );
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "Animal".to_string(),
        }
    );
    assert_eq!(
        *kinds[2],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "double".to_string(),
        }
    );
    assert_eq!(
        *kinds[3],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "long[]".to_string(),
        }
    );
}

#[test]
pub fn extension_methods_resolve_with_scoping() {
    checked!(
        check,
        r#"
        public class Animal { }
        public class Dog : Animal { }

        public static class Extensions
        {
            public static int Doubled(this int value) { return value * 2; }
            public static T LastOf<T>(this T[] items) { return default; }
            public static string Show(this Animal animal) { return "animal"; }
        }

        public class App
        {
            void Run(int[] numbers, Dog dog)
            {
                int d = 5.Doubled();
                string byReceiver = dog.Show();
                string probe = numbers.LastOf();
                dog.Missing();
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    // the probe proves LastOf inferred T = int through the receiver
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
    assert!(matches!(kinds[1], SemanticErrorKind::UnknownMember { .. }));
}

#[test]
pub fn extension_methods_respect_using_scopes() {
    checked!(
        check,
        "namespace Lib { public static class E { public static int Twice(this int v) { return v + v; } } }",
        "using Lib;\npublic class UsesIt { void Run() { int x = 3.Twice(); } }",
        "public class LacksIt { void Run() { int y = 3.Twice(); } }",
    );

    // only the file without `using Lib;` fails
    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert!(matches!(kinds[0], SemanticErrorKind::UnknownMember { .. }));
    assert_eq!(check.errors[0].file, FileId(2));
}

#[test]
pub fn instance_methods_win_over_extensions() {
    checked!(
        check,
        r#"
        public static class Extensions
        {
            public static string Describe(this Thing thing) { return "extension"; }
        }
        public class Thing
        {
            public int Describe() { return 1; }
        }
        public class App
        {
            void Run(Thing thing)
            {
                string probe = thing.Describe();
            }
        }
        "#,
    );

    // the probe reports int: the instance method was chosen
    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert_eq!(
        *kinds[0],
        SemanticErrorKind::TypeMismatch {
            expected: "string".to_string(),
            found: "int".to_string(),
        }
    );
}

#[test]
pub fn target_typed_new_and_default() {
    checked!(
        check,
        r#"
        public class Config
        {
            public int retries;
            public Config() { }
            public Config(int retries) { this.retries = retries; }
        }
        public class App
        {
            void Run()
            {
                Config a = new();
                Config b = new(3);
                int c = default;
                Config d = new("wrong");
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert!(matches!(kinds[0], SemanticErrorKind::NoMatchingOverload));
}

#[test]
pub fn enum_members_surface_without_signatures() {
    declarations!(declarations, "public enum Color { Red, Green }");
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &mock,
    };

    let color = named_source(find(&declarations, "Color"), Vec::new());
    let red = system.members_named(&color, "Red");
    assert_eq!(red.len(), 1);
    assert_eq!(red[0].kind, SymbolKind::EnumMember);
    assert!(red[0].is_static);
    assert_eq!(red[0].signature, None);
}

#[test]
pub fn sibling_types_resolve_across_files_in_one_namespace() {
    declarations!(
        declarations,
        "namespace Game { public partial class Player { public Weapon weapon; } }",
        "namespace Game { public class Weapon {} }",
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    let weapon = find(&declarations, "Game.Weapon");
    assert_eq!(
        member_signature(&declarations, &signatures, "Game.Player", "weapon"),
        MemberSignature::Field(Type::Named {
            target: TypeTarget::Source(weapon),
            arguments: Vec::new(),
        })
    );
}

#[test]
pub fn local_functions_bind_like_methods() {
    checked!(
        check,
        r#"
        public class Body
        {
            int Run(int seed)
            {
                // called above its own declaration
                int later = Double(seed);
                int Double(int x) { return x * 2; }
                // and sharing a variable written above it
                int factor = 3;
                int Scaled() { return later * factor; }
                return Scaled() + Twice(later);
                int Twice(int x) => Double(x);
            }
        }
        "#,
    );
    assert_eq!(error_kinds(&check), Vec::<&SemanticErrorKind>::new());
}

#[test]
pub fn a_local_function_is_neither_generic_nor_static_and_capturing() {
    checked!(
        check,
        r#"
        public class Body
        {
            void Run()
            {
                T Pick<T>(T a) { return a; }
                int total = 0;
                static void Add() { total = total + 1; }
                Add();
                // CS0841: `late` is not a variable yet where `Early` stands
                void Early() { total = late; }
                int late = 2;
                Early();
            }
        }
        "#,
    );
    let kinds = error_kinds(&check);
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, SemanticErrorKind::GenericLocalFunction)),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            SemanticErrorKind::StaticLocalFunctionCapture { name } if name == "total"
        )),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            SemanticErrorKind::LocalUsedBeforeDeclaration { name } if name == "late"
        )),
        "{kinds:?}"
    );
}

#[test]
pub fn a_switch_must_cover_every_case_of_a_union_enum_or_bool() {
    checked!(
        check,
        r#"
        [Union] public abstract class Shape { }
        public sealed class Circle : Shape { public int R; }
        public sealed class Square : Shape { public int S; }
        public abstract class Polygon : Shape { }
        public sealed class Triangle : Polygon { }
        public enum Color { Red, Green, Blue, Azure = 2 }

        [Union] public abstract class Option<T> { }
        public sealed class Some<T> : Option<T> { public T Value; }
        public sealed class None<T> : Option<T> { }

        [Union] public class NotAbstract { }

        public class Body
        {
            int Run(Shape shape, Color color, bool flag, Option<int> option)
            {
                int a = shape switch { Circle c => 1, Square s => 2, Triangle t => 3 };
                int b = shape switch { Circle c => 1, Polygon p => 2 };
                int c = color switch { Color.Red => 1, Color.Green => 2, Color.Blue => 3 };
                int d = color switch { Color.Red => 1, Color.Green => 2 };
                int e = flag switch { true => 1, false => 0 };
                int f = flag switch { true => 1 };
                switch (shape) { case Circle x: break; case Square y: break; }
                switch (shape) { case Circle x: break; default: break; }
                int g = shape switch { Circle { R: > 0 } => 1, Square { S: var s } => 2, Polygon => 3, _ => 4 };
                int h = shape switch { Circle x when x.R > 0 => 1, Square y => 2, Triangle z => 3 };
                int i = shape switch { not Circle => 1, Circle x => 2 };
                int j = shape switch { Circle { R: var r } => r, Square { S: int s } => s, Polygon p => 0 };
                int k = option switch { Some<int> some => some.Value, None<int> none => 0 };
                int l = option switch { Some<int> some => some.Value };
                switch (color) { case Color.Red: break; }
                return a + b + c + d + e + f + g + h + i + j + k + l;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 7, "{kinds:?}");
    let missing = |kind: &SemanticErrorKind| match kind {
        SemanticErrorKind::NonExhaustiveSwitch { missing, .. } => missing.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(*kinds[0], SemanticErrorKind::UnionNotAbstract);
    assert_eq!(missing(kinds[1]), vec!["Square".to_string()]);
    assert_eq!(missing(kinds[2]), vec!["Color.Blue".to_string()]);
    assert_eq!(missing(kinds[3]), vec!["false".to_string()]);
    assert_eq!(missing(kinds[4]), vec!["Triangle".to_string()]);
    assert_eq!(missing(kinds[5]), vec!["Circle".to_string()]);
    assert_eq!(missing(kinds[6]), vec!["None<int>".to_string()]);
}

#[test]
pub fn records_desugar_to_members_and_with_needs_a_record() {
    checked!(
        check,
        r#"
        public record Point(int X, int Y);
        public record struct Cell(int Row);
        public record Labelled(string Label, int X, int Y) : Point(X, Y);
        public class NotARecord(int x) { }

        public class Body
        {
            int Run(Point p, string s, Cell c)
            {
                var q = p with { X = 1 };
                var t = s with { Length = 1 };
                var (a, b) = p;
                c.Row = 2;
                var l = new Labelled("l", 1, 2);
                Point copy = new Point(1, 2) with { Y = 3 };
                p.Deconstruct(out int i, out int j);
                return p.X + q.Y + a + b + l.X + copy.Y + i + j + c.Row;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(matches!(kinds[0], SemanticErrorKind::UnsupportedExpression));
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::WithNeedsRecord {
            type_name: "string".to_string()
        }
    );
}

#[test]
pub fn rectangular_arrays_take_one_index_per_dimension_and_even_rows() {
    checked!(
        check,
        r#"
        public class Body
        {
            int[,] grid = { { 1, 2 }, { 3, 4 } };
            int[,] ragged = { { 1, 2 }, { 3 } };

            int Run(int[] flat)
            {
                int ok = grid[1, 0] + flat[0];
                var made = new int[2, 3];
                var written = new int[,] { { 1 }, { 2 } };
                int[,,] cube = new int[1, 2, 3];
                cube[0, 1, 2] = written[1, 0];
                int wrong = grid[1];
                int alsoWrong = flat[1, 2];
                return ok + made[0, 0] + wrong + alsoWrong;
            }
        }
        "#,
    );

    let kinds = error_kinds(&check);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert!(matches!(
        kinds[0],
        SemanticErrorKind::RaggedArrayInitializer
    ));
    assert_eq!(
        *kinds[1],
        SemanticErrorKind::WrongNumberOfIndices { expected: 2 }
    );
    assert_eq!(
        *kinds[2],
        SemanticErrorKind::WrongNumberOfIndices { expected: 1 }
    );
}

#[test]
pub fn global_usings_are_visible_compilation_wide() {
    declarations!(
        declarations,
        "global using System;\nusing App.Helpers;\nclass A {}",
        "class B {}",
    );

    let globals: Vec<_> = declarations.global_usings().collect();
    assert_eq!(globals.len(), 1);
    assert_eq!(globals[0].0, FileId(0));
}
