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

#[test]
fn thread_count_setting_is_respected() {
    let compiler = Compiler::new(CompilerSettings {
        thread_count: Some(3),
    })
    .unwrap();
    assert_eq!(compiler.thread_count(), 3);
}
