//! The Udon assembly model: data symbols, instructions, address layout, and
//! `.uasm` text emission.
//!
//! Udon programs have two halves. The *data* section declares typed heap slots
//! (every constant, global, argument and temporary lives here — the VM has no
//! registers and its stack holds only heap addresses). The *code* section is a
//! flat instruction stream; each opcode occupies 4 bytes plus 4 bytes for its
//! operand when it has one:
//!
//! | opcode          | operand              | size |
//! |-----------------|----------------------|------|
//! | NOP, POP, COPY  | —                    | 4    |
//! | PUSH            | heap address         | 8    |
//! | JUMP            | code address         | 8    |
//! | JUMP_IF_FALSE   | code address         | 8    |
//! | JUMP_INDIRECT   | heap address         | 8    |
//! | EXTERN          | signature string     | 8    |
//!
//! Layout matters because jump targets are byte addresses. [`Program::assemble`]
//! resolves labels to addresses; the same layout is shared by the text emitter
//! and the emulator, so what runs in tests is byte-for-byte what Unity loads.
//!
//! Initial heap values need care: the stock `.uasm` text format only expresses
//! `null` initial values, so [`Program::to_uasm`] emits `null` for everything
//! and the real values ride along in [`Program::to_meta_json`], which the Unity
//! importer (and the emulator) applies after assembling.

use std::collections::HashMap;
use std::fmt::Write;

/// Jumping here (or past the end of code) halts the program. This is the same
/// convention Udon itself uses for "return from event".
pub const HALT_ADDRESS: u32 = 0xFFFF_FFFC;

/// Index into [`Program::data`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DataId(pub usize);

/// Index into [`Program::labels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LabelId(pub usize);

/// The declared Udon type of a heap slot, e.g. `SystemInt32`,
/// `SystemObjectArray`. Plain strings: the set is open (any whitelisted type).
pub type UdonType = String;

/// An initial heap value. `CodeAddress` becomes a `SystemUInt32` holding the
/// resolved byte address of a label — the raw material of indirect jumps.
#[derive(Debug, Clone, PartialEq)]
pub enum HeapInit {
    Null,
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    UInt32(u32),
    Single(f32),
    Double(f64),
    Char(char),
    Str(String),
    /// The mangled Udon name of a type, for `System.Type` constants.
    TypeOf(String),
    CodeAddress(LabelId),
    /// The assembler's `this` literal. Udon has no `this` pointer and no
    /// extern that hands a program its own object, so self references are
    /// data, not code: the assembler turns `this` into an unresolved heap
    /// reference and the UdonBehaviour replaces it, before the first event,
    /// with the thing of this slot's declared type on its own GameObject —
    /// the GameObject for `%UnityEngineGameObject`, its Transform for
    /// `%UnityEngineTransform`, `GetComponent` otherwise.
    ///
    /// Unlike every other initial value this one *must* travel in the `.uasm`
    /// text: only the SDK's assembler can build that reference object.
    SelfReference,
}

#[derive(Debug, Clone)]
pub struct DataSymbol {
    pub name: String,
    pub udon_type: UdonType,
    pub init: HeapInit,
    /// Exported symbols are visible/editable in the Unity inspector.
    pub export: bool,
    /// Network sync mode (`none`, `linear`, `smooth`) when synced.
    pub sync: Option<String>,
}

/// Jump destination: a label inside the program, or a raw address (used for
/// [`HALT_ADDRESS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Label(LabelId),
    Address(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Pseudo-instruction: marks the position of a label. Zero bytes.
    Label(LabelId),
    /// Pseudo-instruction: annotation for readable dumps. Zero bytes.
    Comment(String),
    Nop,
    /// Push the address of a heap slot onto the VM stack.
    Push(DataId),
    Pop,
    /// Pops destination then source; copies the source slot's value into the
    /// destination slot. (Push source first, then destination.)
    Copy,
    Jump(Target),
    /// Pops one address; jumps when that slot holds `false`.
    JumpIfFalse(Target),
    /// Jumps to the code address stored in the given heap slot.
    JumpIndirect(DataId),
    /// Calls a whitelisted host function. Pops one address per parameter
    /// (pushed in declaration order, out-parameter last).
    Extern(String),
}

impl Op {
    pub fn byte_size(&self) -> u32 {
        match self {
            Op::Label(_) | Op::Comment(_) => 0,
            Op::Nop | Op::Pop | Op::Copy => 4,
            Op::Push(_)
            | Op::Jump(_)
            | Op::JumpIfFalse(_)
            | Op::JumpIndirect(_)
            | Op::Extern(_) => 8,
        }
    }
}

/// An entry point: an exported label the host can invoke as an event
/// (`_start`, `_update`, `_interact`, or a custom event name).
#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub name: String,
    pub label: LabelId,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub data: Vec<DataSymbol>,
    pub code: Vec<Op>,
    /// Human-readable label names, indexed by [`LabelId`]. Purely diagnostic.
    pub labels: Vec<String>,
    pub entry_points: Vec<EntryPoint>,
    /// How the whole behaviour synchronises: `continuous`, `manual` or `none`.
    /// Unlike `.sync` on a variable this is not part of the program at all —
    /// it is a setting on the UdonBehaviour component, so it travels in the
    /// sidecar for the importer to apply.
    pub sync_mode: Option<String>,
    /// The source file this program was compiled from, when the caller knows
    /// it. Carried into the sidecar for tooling: the Unity inspector uses it to
    /// find the other behaviours declared beside this one, which it cannot ask
    /// Unity about (a `.cs` asset only ever maps to the class named after it).
    pub source: Option<String>,
}

impl Program {
    pub fn add_data(&mut self, symbol: DataSymbol) -> DataId {
        self.data.push(symbol);
        DataId(self.data.len() - 1)
    }

    pub fn add_label(&mut self, name: impl Into<String>) -> LabelId {
        self.labels.push(name.into());
        LabelId(self.labels.len() - 1)
    }

    /// Resolves labels to byte addresses. Fails on labels that were created
    /// but never placed.
    pub fn assemble(&self) -> Result<Assembled, AssembleError> {
        let mut label_addresses = vec![None; self.labels.len()];
        let mut address = 0u32;
        for op in &self.code {
            if let Op::Label(label) = op {
                if label_addresses[label.0].is_some() {
                    return Err(AssembleError::DuplicateLabel(self.labels[label.0].clone()));
                }
                label_addresses[label.0] = Some(address);
            }
            address += op.byte_size();
        }

        let resolve = |target: &Target| -> Result<u32, AssembleError> {
            match target {
                Target::Address(address) => Ok(*address),
                Target::Label(label) => label_addresses[label.0]
                    .ok_or_else(|| AssembleError::UnplacedLabel(self.labels[label.0].clone())),
            }
        };

        let mut instructions = Vec::new();
        let mut address = 0u32;
        for op in &self.code {
            let size = op.byte_size();
            let resolved = match op {
                Op::Label(_) | Op::Comment(_) => None,
                Op::Nop => Some(Resolved::Nop),
                Op::Pop => Some(Resolved::Pop),
                Op::Copy => Some(Resolved::Copy),
                Op::Push(data) => Some(Resolved::Push(*data)),
                Op::Jump(target) => Some(Resolved::Jump(resolve(target)?)),
                Op::JumpIfFalse(target) => Some(Resolved::JumpIfFalse(resolve(target)?)),
                Op::JumpIndirect(data) => Some(Resolved::JumpIndirect(*data)),
                Op::Extern(signature) => Some(Resolved::Extern(signature.clone())),
            };
            if let Some(instruction) = resolved {
                instructions.push((address, instruction));
            }
            address += size;
        }

        let mut entry_addresses = HashMap::new();
        for entry in &self.entry_points {
            let address = label_addresses[entry.label.0]
                .ok_or_else(|| AssembleError::UnplacedLabel(entry.name.clone()))?;
            entry_addresses.insert(entry.name.clone(), address);
        }

        Ok(Assembled {
            instructions,
            entry_addresses,
            end_address: address,
            label_addresses: label_addresses
                .into_iter()
                .map(|a| a.unwrap_or(HALT_ADDRESS))
                .collect(),
        })
    }

    /// The `.uasm` text Unity's assembler consumes. Jumps are hex addresses
    /// (universally accepted); initial values are all `null` — real values
    /// live in [`Program::to_meta_json`].
    ///
    /// Only entry-point labels appear as label lines: VRChat's assembler
    /// turns every label into a symbol and rejects two symbols at the same
    /// address (`AliasedSymbolException`), which internal control-flow labels
    /// routinely produce (an `if` with an empty `else` puts its else and end
    /// labels at one address). Internal jumps don't need names anyway — they
    /// are emitted as resolved addresses.
    pub fn to_uasm(&self) -> Result<String, AssembleError> {
        let assembled = self.assemble()?;
        // label id → exported event name (the label line must match `.export`)
        let entry_labels: HashMap<usize, &str> = self
            .entry_points
            .iter()
            .map(|entry| (entry.label.0, entry.name.as_str()))
            .collect();
        let mut out = String::new();

        out.push_str(".data_start\n");
        for symbol in &self.data {
            // both, when both apply: `.export` is what puts the variable in
            // the inspector, `.sync` is what puts it on the network — a synced
            // public variable needs each
            if symbol.export {
                let _ = writeln!(out, "    .export {}", symbol.name);
            }
            if let Some(sync) = &symbol.sync {
                let _ = writeln!(out, "    .sync {}, {}", symbol.name, sync);
            }
            let value = match symbol.init {
                HeapInit::SelfReference => "this",
                _ => "null",
            };
            let _ = writeln!(out, "    {}: %{}, {value}", symbol.name, symbol.udon_type);
        }
        out.push_str(".data_end\n.code_start\n");

        for entry in &self.entry_points {
            let _ = writeln!(out, "    .export {}", entry.name);
        }
        let mut address = 0u32;
        let mut pending_labels: Vec<&str> = Vec::new();
        for op in &self.code {
            match op {
                Op::Label(label) => {
                    if let Some(name) = entry_labels.get(&label.0) {
                        pending_labels.push(name);
                    }
                }
                Op::Comment(_) => {}
                _ => {
                    for label in pending_labels.drain(..) {
                        let _ = writeln!(out, "    {label}:");
                    }
                    let text = match op {
                        Op::Nop => "NOP".to_string(),
                        Op::Pop => "POP".to_string(),
                        Op::Copy => "COPY".to_string(),
                        Op::Push(data) => format!("PUSH, {}", self.data[data.0].name),
                        Op::Jump(target) => {
                            format!("JUMP, 0x{:08X}", self.resolve_for_text(&assembled, target))
                        }
                        Op::JumpIfFalse(target) => format!(
                            "JUMP_IF_FALSE, 0x{:08X}",
                            self.resolve_for_text(&assembled, target)
                        ),
                        Op::JumpIndirect(data) => {
                            format!("JUMP_INDIRECT, {}", self.data[data.0].name)
                        }
                        Op::Extern(signature) => format!("EXTERN, \"{signature}\""),
                        Op::Label(_) | Op::Comment(_) => unreachable!(),
                    };
                    let _ = writeln!(out, "        {text}");
                    address += op.byte_size();
                }
            }
        }
        let _ = address;
        out.push_str(".code_end\n");
        Ok(out)
    }

    fn resolve_for_text(&self, assembled: &Assembled, target: &Target) -> u32 {
        match target {
            Target::Address(address) => *address,
            Target::Label(label) => assembled.label_addresses[label.0],
        }
    }

    /// Sidecar JSON: initial heap values (which `.uasm` cannot express) and
    /// entry points. The Unity importer applies this after assembling; the
    /// emulator applies the same values, so both worlds start identically.
    pub fn to_meta_json(&self) -> Result<String, AssembleError> {
        let assembled = self.assemble()?;
        let mut out = String::from("{\n  \"heap\": [\n");
        let mut first = true;
        for symbol in &self.data {
            // `null` needs no entry, and `this` is already in the `.uasm`
            // text — re-applying it here would overwrite what the
            // UdonBehaviour resolved
            if matches!(symbol.init, HeapInit::Null | HeapInit::SelfReference) {
                continue;
            }
            if !first {
                out.push_str(",\n");
            }
            first = false;
            let (kind, value) = match &symbol.init {
                HeapInit::Null | HeapInit::SelfReference => unreachable!(),
                HeapInit::Boolean(v) => ("Boolean", v.to_string()),
                HeapInit::Int32(v) => ("Int32", v.to_string()),
                HeapInit::Int64(v) => ("Int64", v.to_string()),
                HeapInit::UInt32(v) => ("UInt32", v.to_string()),
                HeapInit::Single(v) => ("Single", format!("{v:?}")),
                HeapInit::Double(v) => ("Double", format!("{v:?}")),
                HeapInit::Char(v) => ("Char", (*v as u32).to_string()),
                HeapInit::Str(v) => ("String", json_string(v)),
                HeapInit::TypeOf(v) => ("Type", json_string(v)),
                HeapInit::CodeAddress(label) => {
                    ("UInt32", assembled.label_addresses[label.0].to_string())
                }
            };
            let _ = write!(
                out,
                "    {{\"name\": {}, \"kind\": \"{}\", \"value\": {}}}",
                json_string(&symbol.name),
                kind,
                if matches!(symbol.init, HeapInit::Str(_) | HeapInit::TypeOf(_)) {
                    value
                } else {
                    format!("\"{value}\"")
                }
            );
        }
        out.push_str("\n  ],\n  \"entryPoints\": [");
        let mut first = true;
        for entry in &self.entry_points {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(&json_string(&entry.name));
        }
        out.push(']');
        if let Some(mode) = &self.sync_mode {
            let _ = write!(out, ",\n  \"syncMode\": {}", json_string(mode));
        }
        if let Some(source) = &self.source {
            let _ = write!(out, ",\n  \"source\": {}", json_string(source));
        }
        out.push_str("\n}\n");
        Ok(out)
    }

    /// Human-oriented dump with labels and comments inline, for debugging.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        let mut address = 0u32;
        for op in &self.code {
            match op {
                Op::Label(label) => {
                    let _ = writeln!(out, "{}:", self.labels[label.0]);
                }
                Op::Comment(text) => {
                    let _ = writeln!(out, "    # {text}");
                }
                _ => {
                    let _ = writeln!(out, "    0x{address:08X}  {op:?}");
                    address += op.byte_size();
                }
            }
        }
        out
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A label-resolved instruction, paired with its byte address.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Nop,
    Pop,
    Copy,
    Push(DataId),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIndirect(DataId),
    Extern(String),
}

#[derive(Debug)]
pub struct Assembled {
    /// `(byte address, instruction)` in program order.
    pub instructions: Vec<(u32, Resolved)>,
    pub entry_addresses: HashMap<String, u32>,
    pub end_address: u32,
    pub label_addresses: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssembleError {
    UnplacedLabel(String),
    DuplicateLabel(String),
}
