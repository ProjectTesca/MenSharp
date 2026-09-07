//! Member lookup through the type system: inheritance and instantiation.

mod common;

use common::{MockExternal, declarations, external, find, named_source};
use men_sharp_semantics::types::{MemberSignature, Type};
use men_sharp_semantics::{MemberOrigin, TypeSystem, resolve_signatures};

#[test]
pub fn inherited_members_come_back_instantiated() {
    declarations!(
        declarations,
        r#"
        public class Base<T>
        {
            public T Value;
            public void SetValue(T value) {}
        }
        public class Derived : Base<int>
        {
            public string Name;
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);

    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &mock,
    };

    let derived = named_source(find(&declarations, "Derived"), Vec::new());
    let int32 = external(mock.id_of("System", "Int32"));

    // Base<T>.Value reached through Derived : Base<int> becomes int
    let value = system.members_named(&derived, "Value");
    assert_eq!(value.len(), 1);
    assert_eq!(
        value[0].signature,
        Some(MemberSignature::Field(int32.clone()))
    );
    assert_eq!(
        value[0].declaring_type,
        named_source(find(&declarations, "Base"), vec![int32.clone()])
    );

    // and so does the parameter of SetValue(T)
    let set_value = system.members_named(&derived, "SetValue");
    let Some(MemberSignature::Function(function)) = &set_value[0].signature else {
        panic!("expected a function");
    };
    assert_eq!(function.parameters[0].parameter_type, int32);

    // its own member is found on itself
    let name = system.members_named(&derived, "Name");
    assert_eq!(name.len(), 1);
    assert_eq!(name[0].declaring_type, derived);
}

#[test]
pub fn lookup_returns_all_candidates_nearest_first() {
    declarations!(
        declarations,
        r#"
        public class A { public void F() {} }
        public class B : A { public static void F(int x) {} }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &mock,
    };

    let b = named_source(find(&declarations, "B"), Vec::new());
    let candidates = system.members_named(&b, "F");

    assert_eq!(candidates.len(), 2);
    // B's own overload first, with its modifiers readable
    assert_eq!(candidates[0].declaring_type, b);
    assert!(candidates[0].is_static);
    assert_eq!(
        candidates[0].accessibility,
        men_sharp_semantics::Accessibility::Public
    );
    assert_eq!(
        candidates[1].declaring_type,
        named_source(find(&declarations, "A"), Vec::new())
    );
    assert!(!candidates[1].is_static);
}

#[test]
pub fn interfaces_and_type_parameters_walk_their_hierarchies() {
    declarations!(
        declarations,
        r#"
        public interface IAnimal { void Speak(); }
        public interface IDog : IAnimal { void Fetch(); }
        public class Kennel<T> where T : IDog
        {
            public void Handle(T dog) {}
        }
        "#,
    );
    let mock = MockExternal::corlib();
    let signatures = resolve_signatures(&declarations, &mock);
    assert_eq!(signatures.errors, vec![]);
    let system = TypeSystem {
        declarations: &declarations,
        signatures: &signatures,
        external: &mock,
    };

    // an interface receiver sees members of the interfaces it extends
    let dog = named_source(find(&declarations, "IDog"), Vec::new());
    assert_eq!(system.members_named(&dog, "Fetch").len(), 1);
    assert_eq!(system.members_named(&dog, "Speak").len(), 1);

    // a constrained type parameter sees its bound's members
    let kennel = find(&declarations, "Kennel");
    let t = declarations.table.symbol(kennel).type_parameters[0];
    let speak = system.members_named(&Type::TypeParameter(t), "Speak");
    assert_eq!(speak.len(), 1);
    assert!(matches!(speak[0].origin, MemberOrigin::Source(_)));
}
