//! End-to-end tests of the driver: the same input must produce the same symbol
//! table and the same diagnostics no matter how many threads the settings ask for.

use men_sharp_compiler::{Compiler, CompilerSettings, SourceCode};

fn sources() -> Vec<SourceCode> {
    vec![
        SourceCode::new(
            "player.cs",
            r#"
            namespace Game
            {
                public partial class Player
                {
                    int health;
                    public void Move(float x, float y) {}
                }
            }
            "#,
        ),
        SourceCode::new(
            "player_net.cs",
            r#"
            namespace Game
            {
                public partial class Player
                {
                    public void Sync() {}
                }
            }
            "#,
        ),
        SourceCode::new(
            "items.cs",
            r#"
            namespace Game.Items;

            public enum Rarity { Common, Rare }

            public class Item
            {
                public Rarity rarity;
                public Rarity rarity;
            }
            "#,
        ),
    ]
}

/// A stable, order-independent fingerprint of a compilation's outcome.
fn fingerprint(thread_count: Option<usize>) -> (Vec<String>, Vec<String>) {
    let compiler = Compiler::new(CompilerSettings { thread_count }).unwrap();
    let files = compiler.parse(sources());
    let declarations = compiler.collect_declarations(&files);

    let mut symbols: Vec<String> = declarations
        .table
        .iter()
        .map(|(id, symbol)| {
            format!(
                "{:?} {} x{}",
                symbol.kind,
                declarations.table.fully_qualified_name(id),
                symbol.declarations.len()
            )
        })
        .collect();
    symbols.sort();

    let errors = declarations
        .errors
        .iter()
        .map(|error| format!("{:?} {:?} {:?}", error.file, error.span, error.kind))
        .collect();

    (symbols, errors)
}

#[test]
fn partial_types_and_namespaces_merge_across_files() {
    let compiler = Compiler::new(CompilerSettings::default()).unwrap();
    let files = compiler.parse(sources());
    let declarations = compiler.collect_declarations(&files);

    let root = declarations.table.root();
    let game = declarations.table.symbol(root).members_named("Game")[0];
    let player = declarations.table.symbol(game).members_named("Player")[0];

    let player = declarations.table.symbol(player);
    assert_eq!(player.declarations.len(), 2);
    assert_eq!(player.members_named("Move").len(), 1);
    assert_eq!(player.members_named("Sync").len(), 1);

    // the duplicate `rarity` field is the only error
    assert_eq!(declarations.errors.len(), 1);
}

#[test]
fn results_do_not_depend_on_the_thread_count() {
    let single = fingerprint(Some(1));
    let quad = fingerprint(Some(4));
    let default = fingerprint(None);

    assert_eq!(single, quad);
    assert_eq!(single, default);
}

/// End to end against the real .NET core library, when this machine has one.
#[test]
fn signatures_resolve_against_a_real_core_library() {
    let corelib = std::path::Path::new("/usr/share/dotnet/shared/Microsoft.NETCore.App");
    let Some(corelib) = std::fs::read_dir(corelib).ok().and_then(|entries| {
        let mut versions: Vec<_> = entries.flatten().collect();
        versions.sort_by_key(|entry| entry.file_name());
        versions
            .pop()
            .map(|entry| entry.path().join("System.Private.CoreLib.dll"))
    }) else {
        eprintln!("skipped: no .NET runtime on this machine");
        return;
    };

    let compiler = Compiler::new(CompilerSettings::default()).unwrap();

    let references = vec![std::fs::read(corelib).unwrap()];
    let references = compiler.load_references(&references).unwrap();

    let files = compiler.parse(vec![SourceCode::new(
        "inventory.cs",
        r#"
        using System.Collections.Generic;

        namespace Game
        {
            public class Inventory
            {
                private List<string> items = new List<string>();
                public int Count => 0;
                public void Add(string item, int quantity) {}
            }
        }
        "#,
    )]);
    let declarations = compiler.collect_declarations(&files);
    let signatures = compiler.resolve_signatures(&declarations, &references);

    assert_eq!(declarations.errors, vec![]);
    assert_eq!(signatures.errors, vec![]);

    let root = declarations.table.root();
    let game = declarations.table.symbol(root).members_named("Game")[0];
    let inventory = declarations.table.symbol(game).members_named("Inventory")[0];
    let inventory = declarations.table.symbol(inventory);

    // items: List<string> — an external generic instantiated with external string
    let items = inventory.members_named("items")[0];
    let men_sharp_semantics::MemberSignature::Field(men_sharp_semantics::Type::Named {
        target: men_sharp_semantics::TypeTarget::External(list),
        arguments,
    }) = signatures.members.get(&items).unwrap().clone()
    else {
        panic!("items should be an external named type");
    };
    assert_eq!(
        references.display_name(list),
        "System.Collections.Generic.List`1"
    );
    assert_eq!(arguments.len(), 1);

    // Add(string, int) — parameters resolved through the real metadata
    let add = inventory.members_named("Add")[0];
    let men_sharp_semantics::MemberSignature::Function(function) =
        signatures.members.get(&add).unwrap().clone()
    else {
        panic!("Add should be a function");
    };
    assert_eq!(function.return_type, men_sharp_semantics::Type::Void);
    assert_eq!(function.parameters.len(), 2);
}

#[test]
fn thread_count_setting_is_respected() {
    let compiler = Compiler::new(CompilerSettings {
        thread_count: Some(3),
    })
    .unwrap();
    assert_eq!(compiler.thread_count(), 3);
}
