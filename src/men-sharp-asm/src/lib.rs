//! Udon assembly, as a Rust data structure: build a [`Program`], resolve it
//! with [`Program::assemble`], run it in the [`emulator`], and ship it to
//! Unity as `.uasm` text plus a heap-initialisation sidecar.
//!
//! This crate knows nothing about M#. The code generator (`men-sharp-codegen`)
//! lowers checked M# programs into this model; hand-written programs work just
//! as well, which is how the emulator itself is tested.

pub mod emulator;
pub mod program;
pub mod world;

pub use emulator::{Alone, DelayedEvent, Due, Emulator, EmulatorError, Peers, Value};
pub use program::{
    AssembleError, Assembled, DataId, DataSymbol, EntryPoint, HALT_ADDRESS, HeapInit, LabelId,
    Layout, NetworkCallable, Op, Program, Resolved, SourceMark, SourceMarkKind, Target, UdonType,
};
pub use world::World;

#[cfg(test)]
mod tests {
    use super::*;

    /// Small builder so tests read like assembly listings.
    struct Asm {
        program: Program,
    }

    impl Asm {
        fn new() -> Self {
            Asm {
                program: Program::default(),
            }
        }

        fn slot(&mut self, name: &str, udon_type: &str, init: HeapInit) -> DataId {
            self.program.add_data(DataSymbol {
                name: name.to_string(),
                udon_type: udon_type.to_string(),
                init,
                export: false,
                sync: None,
            })
        }

        fn int(&mut self, name: &str, value: i32) -> DataId {
            self.slot(name, "SystemInt32", HeapInit::Int32(value))
        }

        fn label(&mut self, name: &str) -> LabelId {
            self.program.add_label(name)
        }

        fn entry(&mut self, name: &str, label: LabelId) {
            self.program.entry_points.push(EntryPoint {
                name: name.to_string(),
                label,
            });
        }

        fn op(&mut self, op: Op) {
            self.program.code.push(op);
        }

        fn copy(&mut self, source: DataId, destination: DataId) {
            self.op(Op::Push(source));
            self.op(Op::Push(destination));
            self.op(Op::Copy);
        }

        fn call_extern(&mut self, signature: &str, arguments: &[DataId]) {
            for argument in arguments {
                self.op(Op::Push(*argument));
            }
            self.op(Op::Extern(signature.to_string()));
        }

        fn run(&self, entry: &str) -> Emulator {
            let assembled = self.program.assemble().unwrap();
            let mut emulator = Emulator::new(&self.program, &assembled);
            emulator.run(&assembled, entry).unwrap();
            emulator
        }
    }

    const ADD: &str = "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32";
    const LESS: &str = "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean";

    #[test]
    fn straight_line_arithmetic() {
        let mut asm = Asm::new();
        let a = asm.int("a", 20);
        let b = asm.int("b", 22);
        let result = asm.int("result", 0);
        let start = asm.label("_start");
        asm.entry("_start", start);

        asm.op(Op::Label(start));
        asm.call_extern(ADD, &[a, b, result]);

        let emulator = asm.run("_start");
        assert_eq!(emulator.value_of("result").unwrap().as_i32().unwrap(), 42);
    }

    #[test]
    fn loops_run_and_terminate() {
        // total = 0; i = 0; while (i < 10) { total += i; i += 1; }
        let mut asm = Asm::new();
        let total = asm.int("total", 0);
        let i = asm.int("i", 0);
        let one = asm.int("one", 1);
        let ten = asm.int("ten", 10);
        let condition = asm.slot("condition", "SystemBoolean", HeapInit::Boolean(false));

        let start = asm.label("_start");
        let head = asm.label("loop_head");
        let exit = asm.label("loop_exit");
        asm.entry("_start", start);

        asm.op(Op::Label(start));
        asm.op(Op::Label(head));
        asm.call_extern(LESS, &[i, ten, condition]);
        asm.op(Op::Push(condition));
        asm.op(Op::JumpIfFalse(Target::Label(exit)));
        asm.call_extern(ADD, &[total, i, total]);
        asm.call_extern(ADD, &[i, one, i]);
        asm.op(Op::Jump(Target::Label(head)));
        asm.op(Op::Label(exit));

        let emulator = asm.run("_start");
        assert_eq!(emulator.value_of("total").unwrap().as_i32().unwrap(), 45);
    }

    #[test]
    fn function_call_via_indirect_return() {
        // The calling convention codegen uses: the caller stores a return
        // address constant into the callee's return slot, jumps to the callee;
        // the callee ends with JUMP_INDIRECT on that slot.
        let mut asm = Asm::new();
        let argument = asm.int("double__arg", 0);
        let result = asm.int("double__result", 0);
        let return_slot = asm.slot("double__return", "SystemUInt32", HeapInit::UInt32(0));
        let twenty_one = asm.int("twenty_one", 21);
        let answer = asm.int("answer", 0);

        let start = asm.label("_start");
        let double = asm.label("fn_double");
        let continuation = asm.label("after_call");
        let return_constant = asm.slot(
            "const_after_call",
            "SystemUInt32",
            HeapInit::CodeAddress(continuation),
        );
        asm.entry("_start", start);

        asm.op(Op::Label(start));
        asm.copy(twenty_one, argument);
        asm.copy(return_constant, return_slot);
        asm.op(Op::Jump(Target::Label(double)));
        asm.op(Op::Label(continuation));
        asm.copy(result, answer);
        asm.op(Op::Jump(Target::Address(HALT_ADDRESS)));

        // int double(int x) => x + x;
        asm.op(Op::Label(double));
        asm.call_extern(ADD, &[argument, argument, result]);
        asm.op(Op::JumpIndirect(return_slot));

        let emulator = asm.run("_start");
        assert_eq!(emulator.value_of("answer").unwrap().as_i32().unwrap(), 42);
    }

    #[test]
    fn object_array_externs_model_objects() {
        // new object[2]; obj[0] = 7; obj[1] = "x"; read back obj[0]
        let mut asm = Asm::new();
        let size = asm.int("size", 2);
        let object = asm.slot("object", "SystemObjectArray", HeapInit::Null);
        let zero = asm.int("zero", 0);
        let seven = asm.int("seven", 7);
        let out = asm.int("out", 0);

        let start = asm.label("_start");
        asm.entry("_start", start);
        asm.op(Op::Label(start));
        asm.call_extern(
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
            &[size, object],
        );
        asm.call_extern(
            "SystemObjectArray.__Set__SystemInt32_SystemObject__SystemVoid",
            &[object, zero, seven],
        );
        asm.call_extern(
            "SystemObjectArray.__Get__SystemInt32__SystemObject",
            &[object, zero, out],
        );

        let emulator = asm.run("_start");
        assert_eq!(emulator.value_of("out").unwrap().as_i32().unwrap(), 7);
    }

    #[test]
    fn infinite_loops_run_out_of_fuel() {
        let mut asm = Asm::new();
        let start = asm.label("_start");
        asm.entry("_start", start);
        asm.op(Op::Label(start));
        asm.op(Op::Jump(Target::Label(start)));

        let assembled = asm.program.assemble().unwrap();
        let mut emulator = Emulator::new(&asm.program, &assembled);
        emulator.fuel = 1000;
        assert_eq!(
            emulator.run(&assembled, "_start"),
            Err(EmulatorError::OutOfFuel)
        );
    }

    #[test]
    fn uasm_text_and_meta_are_emitted() {
        let mut asm = Asm::new();
        let a = asm.int("a", 20);
        let b = asm.int("b", 22);
        let result = asm.int("result", 0);
        asm.program.data[result.0].export = true;
        let start = asm.label("_start");
        asm.entry("_start", start);
        asm.op(Op::Label(start));
        asm.call_extern(ADD, &[a, b, result]);
        asm.op(Op::Jump(Target::Address(HALT_ADDRESS)));

        let text = asm.program.to_uasm().unwrap();
        assert!(text.contains(".data_start"));
        assert!(text.contains("    .export result"));
        assert!(text.contains("a: %SystemInt32, null"));
        assert!(text.contains(".export _start"));
        assert!(text.contains(&format!("EXTERN, \"{ADD}\"")));
        assert!(text.contains("JUMP, 0xFFFFFFFC"));

        let meta = asm.program.to_meta_json().unwrap();
        assert!(meta.contains("\"name\": \"a\", \"kind\": \"Int32\", \"value\": \"20\""));
        assert!(meta.contains("\"entryPoints\": [\"_start\"]"));
    }

    #[test]
    fn self_references_travel_in_the_text_not_the_sidecar() {
        // only the SDK's assembler can build the unresolved heap reference the
        // UdonBehaviour later swaps for its own GameObject, so `this` is the
        // one initial value that must be in the `.uasm` — and re-applying it
        // from the sidecar would clobber what the UdonBehaviour resolved
        let mut asm = Asm::new();
        asm.slot(
            "__this_gameObject",
            "UnityEngineGameObject",
            HeapInit::SelfReference,
        );
        asm.int("count", 3);
        let start = asm.label("_start");
        asm.entry("_start", start);
        asm.op(Op::Label(start));
        asm.op(Op::Jump(Target::Address(HALT_ADDRESS)));

        let text = asm.program.to_uasm().unwrap();
        assert!(text.contains("__this_gameObject: %UnityEngineGameObject, this"));

        let meta = asm.program.to_meta_json().unwrap();
        assert!(!meta.contains("__this_gameObject"), "{meta}");
        assert!(meta.contains("\"name\": \"count\""));
    }

    #[test]
    fn jump_addresses_follow_the_4_8_byte_layout() {
        let mut asm = Asm::new();
        let a = asm.int("a", 1);
        let start = asm.label("_start");
        let target = asm.label("target");
        asm.entry("_start", start);
        asm.op(Op::Label(start));
        asm.op(Op::Push(a)); // 0x00, 8 bytes
        asm.op(Op::Pop); // 0x08, 4 bytes
        asm.op(Op::Jump(Target::Label(target))); // 0x0C, 8 bytes
        asm.op(Op::Label(target)); // = 0x14
        asm.op(Op::Nop);

        let assembled = asm.program.assemble().unwrap();
        assert_eq!(assembled.label_addresses[target.0], 0x14);
        let text = asm.program.to_uasm().unwrap();
        assert!(text.contains("JUMP, 0x00000014"));
    }

    #[test]
    fn uasm_emits_only_entry_labels_and_by_event_name() {
        // VRChat's assembler rejects two labels at one address, which empty
        // else-branches produce constantly — so internal labels must never
        // appear in the text, and entry labels must use the exported name.
        let mut asm = Asm::new();
        let a = asm.int("a", 1);
        let start = asm.label("event__start"); // internal name differs
        let alias_a = asm.label("if_else");
        let alias_b = asm.label("if_end");
        asm.entry("_start", start);

        asm.op(Op::Label(start));
        asm.op(Op::Push(a));
        asm.op(Op::Pop);
        asm.op(Op::Label(alias_a)); // both at the same address
        asm.op(Op::Label(alias_b));
        asm.op(Op::Nop);

        let text = asm.program.to_uasm().unwrap();
        assert!(text.contains("    .export _start\n"));
        assert!(text.contains("    _start:\n"));
        assert!(!text.contains("event__start:"));
        assert!(!text.contains("if_else"));
        assert!(!text.contains("if_end"));
    }
}
