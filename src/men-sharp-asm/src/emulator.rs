//! A miniature Udon VM, for testing generated programs without Unity.
//!
//! The interpreter is faithful to the real machine where it matters: the stack
//! holds heap addresses, `COPY` moves values between typed slots, `EXTERN`
//! pops one address per parameter (declaration order, out-parameter last), and
//! jumping to [`HALT_ADDRESS`] or past the end of code stops execution.
//!
//! Externs are Rust stubs implementing the same *observable* behaviour as the
//! whitelisted .NET methods. The registry is deliberately small and grows on
//! demand — hitting [`EmulatorError::UnknownExtern`] in a test is the signal
//! to add one. Whether a signature exists in the real whitelist is a separate
//! question, checked against the SDK dump by the codegen crate's tests.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::program::{Assembled, HALT_ADDRESS, HeapInit, Program, Resolved};

/// A runtime value in a heap slot. `Array` covers every array shape,
/// including the `object[]` instances of the M# object model.
#[derive(Debug, Clone, Default)]
pub enum Value {
    #[default]
    Null,
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    UInt32(u32),
    Single(f32),
    Double(f64),
    Char(char),
    Str(Rc<str>),
    Array(Rc<RefCell<Vec<Value>>>),
    /// A `System.Type` value, carried as the mangled Udon type name.
    Type(Rc<str>),
    /// What a `HeapInit::SelfReference` slot holds: on Udon the UdonBehaviour
    /// resolves it to the GameObject/Transform/component it is attached to.
    /// The emulator has no scene, so it keeps the requested Udon type name and
    /// lets any extern that touches it fail loudly rather than pretend.
    SelfComponent(Rc<str>),
}

impl Value {
    pub fn as_i32(&self) -> Result<i32, EmulatorError> {
        match self {
            Value::Int32(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected Int32, found {other:?}"
            ))),
        }
    }

    pub fn as_bool(&self) -> Result<bool, EmulatorError> {
        match self {
            Value::Boolean(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected Boolean, found {other:?}"
            ))),
        }
    }

    // strict on purpose: the real heap refuses an Int32 slot read as Int64,
    // and this is what catches a missing numeric promotion
    pub fn as_i64(&self) -> Result<i64, EmulatorError> {
        match self {
            Value::Int64(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected Int64, found {other:?}"
            ))),
        }
    }

    pub fn as_f64(&self) -> Result<f64, EmulatorError> {
        match self {
            Value::Double(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected Double, found {other:?}"
            ))),
        }
    }

    pub fn as_u32(&self) -> Result<u32, EmulatorError> {
        match self {
            Value::UInt32(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected UInt32, found {other:?}"
            ))),
        }
    }

    pub fn as_f32(&self) -> Result<f32, EmulatorError> {
        match self {
            Value::Single(v) => Ok(*v),
            other => Err(EmulatorError::TypeError(format!(
                "expected Single, found {other:?}"
            ))),
        }
    }

    pub fn as_array(&self) -> Result<Rc<RefCell<Vec<Value>>>, EmulatorError> {
        match self {
            Value::Array(v) => Ok(v.clone()),
            other => Err(EmulatorError::TypeError(format!(
                "expected an array, found {other:?}"
            ))),
        }
    }

    pub fn as_str(&self) -> Result<Rc<str>, EmulatorError> {
        match self {
            Value::Str(v) => Ok(v.clone()),
            other => Err(EmulatorError::TypeError(format!(
                "expected String, found {other:?}"
            ))),
        }
    }

    /// `ToString` semantics for the types the stub library supports.
    pub fn display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Boolean(v) => {
                if *v {
                    "True".into()
                } else {
                    "False".into()
                }
            }
            Value::Int32(v) => v.to_string(),
            Value::Int64(v) => v.to_string(),
            Value::UInt32(v) => v.to_string(),
            Value::Single(v) => v.to_string(),
            Value::Double(v) => v.to_string(),
            Value::Char(v) => v.to_string(),
            Value::Str(v) => v.to_string(),
            Value::Array(_) => "System.Object[]".into(),
            Value::Type(name) => name.to_string(),
            Value::SelfComponent(udon_type) => format!("<self:{udon_type}>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmulatorError {
    UnknownEntryPoint(String),
    UnknownExtern(String),
    /// An extern threw — on Udon the VM logs it and halts the behaviour.
    /// The compiler's own halt (a failed cast, later `throw`) is one of
    /// these on purpose; the message is what it logged.
    Exception(String),
    StackUnderflow,
    InvalidJump(u32),
    TypeError(String),
    /// The step budget ran out — almost always an unintended infinite loop.
    OutOfFuel,
    IndexOutOfRange {
        index: i32,
        length: usize,
    },
}

pub struct Emulator {
    pub heap: Vec<Value>,
    names: HashMap<String, usize>,
    stack: Vec<usize>,
    /// Everything passed to `UnityEngineDebug.__Log__…`, for assertions.
    pub log: Vec<String>,
    pub fuel: u64,
}

impl Emulator {
    /// Builds an emulator with the program's heap laid out and initial values
    /// applied (the same values `to_meta_json` ships to Unity).
    pub fn new(program: &Program, assembled: &Assembled) -> Self {
        let mut heap = Vec::with_capacity(program.data.len());
        let mut names = HashMap::new();
        for (index, symbol) in program.data.iter().enumerate() {
            names.insert(symbol.name.clone(), index);
            heap.push(match &symbol.init {
                // value-typed slots default to their type's default, exactly
                // as the Unity importer initialises the real heap
                HeapInit::Null => match symbol.udon_type.as_str() {
                    "SystemInt32" => Value::Int32(0),
                    "SystemInt64" => Value::Int64(0),
                    "SystemUInt32" => Value::UInt32(0),
                    "SystemBoolean" => Value::Boolean(false),
                    "SystemSingle" => Value::Single(0.0),
                    "SystemDouble" => Value::Double(0.0),
                    "SystemChar" => Value::Char('\0'),
                    _ => Value::Null,
                },
                HeapInit::Boolean(v) => Value::Boolean(*v),
                HeapInit::Int32(v) => Value::Int32(*v),
                HeapInit::Int64(v) => Value::Int64(*v),
                HeapInit::UInt32(v) => Value::UInt32(*v),
                HeapInit::Single(v) => Value::Single(*v),
                HeapInit::Double(v) => Value::Double(*v),
                HeapInit::Char(v) => Value::Char(*v),
                HeapInit::Str(v) => Value::Str(Rc::from(v.as_str())),
                HeapInit::TypeOf(v) => Value::Type(Rc::from(v.as_str())),
                // enums run as their underlying integral value here; only
                // Unity can build the real boxed value
                HeapInit::EnumValue { value, .. } => Value::Int32(*value as i32),
                HeapInit::CodeAddress(label) => Value::UInt32(assembled.label_addresses[label.0]),
                HeapInit::SelfReference => {
                    Value::SelfComponent(Rc::from(symbol.udon_type.as_str()))
                }
            });
        }
        Emulator {
            heap,
            names,
            stack: Vec::new(),
            log: Vec::new(),
            fuel: 50_000_000,
        }
    }

    /// The value of a named heap slot — how tests read program results.
    pub fn value_of(&self, name: &str) -> Option<&Value> {
        self.names.get(name).map(|&index| &self.heap[index])
    }

    /// Overwrites a named heap slot before running — how tests model the
    /// Unity inspector's public-variable values, which are applied after the
    /// heap loads and before any event fires.
    pub fn set_value(&mut self, name: &str, value: Value) -> bool {
        match self.names.get(name) {
            Some(&index) => {
                self.heap[index] = value;
                true
            }
            None => false,
        }
    }

    pub fn run(&mut self, assembled: &Assembled, entry: &str) -> Result<(), EmulatorError> {
        let start = *assembled
            .entry_addresses
            .get(entry)
            .ok_or_else(|| EmulatorError::UnknownEntryPoint(entry.to_string()))?;

        // byte address -> instruction index
        let index_of: HashMap<u32, usize> = assembled
            .instructions
            .iter()
            .enumerate()
            .map(|(index, (address, _))| (*address, index))
            .collect();

        let jump_to = |address: u32| -> Result<Option<usize>, EmulatorError> {
            if address >= assembled.end_address || address == HALT_ADDRESS {
                return Ok(None); // off the end = halt
            }
            index_of
                .get(&address)
                .copied()
                .map(Some)
                .ok_or(EmulatorError::InvalidJump(address))
        };

        let Some(mut pc) = jump_to(start)? else {
            return Ok(());
        };

        loop {
            if self.fuel == 0 {
                return Err(EmulatorError::OutOfFuel);
            }
            self.fuel -= 1;

            let (_, instruction) = &assembled.instructions[pc];
            match instruction {
                Resolved::Nop => {}
                Resolved::Pop => {
                    self.stack.pop().ok_or(EmulatorError::StackUnderflow)?;
                }
                Resolved::Push(data) => self.stack.push(data.0),
                Resolved::Copy => {
                    let destination = self.stack.pop().ok_or(EmulatorError::StackUnderflow)?;
                    let source = self.stack.pop().ok_or(EmulatorError::StackUnderflow)?;
                    self.heap[destination] = self.heap[source].clone();
                }
                Resolved::Jump(address) => match jump_to(*address)? {
                    Some(index) => {
                        pc = index;
                        continue;
                    }
                    None => return Ok(()),
                },
                Resolved::JumpIfFalse(address) => {
                    let condition = self.stack.pop().ok_or(EmulatorError::StackUnderflow)?;
                    if !self.heap[condition].as_bool()? {
                        match jump_to(*address)? {
                            Some(index) => {
                                pc = index;
                                continue;
                            }
                            None => return Ok(()),
                        }
                    }
                }
                Resolved::JumpIndirect(data) => {
                    let address = self.heap[data.0].as_u32()?;
                    match jump_to(address)? {
                        Some(index) => {
                            pc = index;
                            continue;
                        }
                        None => return Ok(()),
                    }
                }
                Resolved::Extern(signature) => {
                    let signature = signature.clone();
                    self.call_extern(&signature)?;
                }
            }
            pc += 1;
            if pc >= assembled.instructions.len() {
                return Ok(());
            }
        }
    }

    /// Pops `count` argument addresses (they were pushed in declaration
    /// order) and returns them in that order.
    fn pop_arguments(&mut self, count: usize) -> Result<Vec<usize>, EmulatorError> {
        if self.stack.len() < count {
            return Err(EmulatorError::StackUnderflow);
        }
        Ok(self.stack.split_off(self.stack.len() - count))
    }

    fn call_extern(&mut self, signature: &str) -> Result<(), EmulatorError> {
        // Binary integer/float operators and comparisons share one shape:
        // IN a, IN b, OUT result.
        macro_rules! binary_i32 {
            ($op:expr) => {{
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_i32()?;
                let b = self.heap[args[1]].as_i32()?;
                self.heap[args[2]] = $op(a, b);
                return Ok(());
            }};
        }
        macro_rules! binary_f32 {
            ($op:expr) => {{
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_f32()?;
                let b = self.heap[args[1]].as_f32()?;
                self.heap[args[2]] = $op(a, b);
                return Ok(());
            }};
        }

        match signature {
            // ---- Int32 arithmetic ----
            "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a: i32, b: i32| Value::Int32(a.wrapping_add(b)))
            }
            "SystemInt32.__op_Subtraction__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a: i32, b: i32| Value::Int32(a.wrapping_sub(b)))
            }
            "SystemInt32.__op_Multiplication__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a: i32, b: i32| Value::Int32(a.wrapping_mul(b)))
            }
            "SystemInt32.__op_Division__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a: i32, b: i32| Value::Int32(a.wrapping_div(b)))
            }
            "SystemInt32.__op_Modulus__SystemInt32_SystemInt32__SystemInt32"
            | "SystemInt32.__op_Remainder__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a: i32, b: i32| Value::Int32(a.wrapping_rem(b)))
            }
            "SystemInt32.__op_UnaryMinus__SystemInt32__SystemInt32" => {
                let args = self.pop_arguments(2)?;
                let a = self.heap[args[0]].as_i32()?;
                self.heap[args[1]] = Value::Int32(a.wrapping_neg());
                Ok(())
            }
            // ---- Int32 comparisons ----
            "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a < b))
            }
            "SystemInt32.__op_LessThanOrEqual__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a <= b))
            }
            "SystemInt32.__op_GreaterThan__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a > b))
            }
            "SystemInt32.__op_GreaterThanOrEqual__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a >= b))
            }
            "SystemInt32.__op_LogicalAnd__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a, b| Value::Int32(a & b))
            }
            "SystemInt32.__op_LogicalOr__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a, b| Value::Int32(a | b))
            }
            "SystemInt32.__op_LogicalXor__SystemInt32_SystemInt32__SystemInt32" => {
                binary_i32!(|a, b| Value::Int32(a ^ b))
            }
            "SystemInt32.__op_Equality__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a == b))
            }
            "SystemInt32.__op_Inequality__SystemInt32_SystemInt32__SystemBoolean" => {
                binary_i32!(|a, b| Value::Boolean(a != b))
            }
            // ---- Char ----
            "SystemChar.__op_Equality__SystemChar_SystemChar__SystemBoolean"
            | "SystemChar.__op_Inequality__SystemChar_SystemChar__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let equal = match (&self.heap[args[0]], &self.heap[args[1]]) {
                    (Value::Char(a), Value::Char(b)) => a == b,
                    (a, b) => {
                        return Err(EmulatorError::TypeError(format!(
                            "{signature}: {a:?} vs {b:?}"
                        )));
                    }
                };
                self.heap[args[2]] = Value::Boolean(equal == signature.contains("op_Equality"));
                Ok(())
            }
            // ---- Single arithmetic (subset) ----
            "SystemSingle.__op_Addition__SystemSingle_SystemSingle__SystemSingle" => {
                binary_f32!(|a: f32, b: f32| Value::Single(a + b))
            }
            "SystemSingle.__op_Subtraction__SystemSingle_SystemSingle__SystemSingle" => {
                binary_f32!(|a: f32, b: f32| Value::Single(a - b))
            }
            "SystemSingle.__op_Multiplication__SystemSingle_SystemSingle__SystemSingle" => {
                binary_f32!(|a: f32, b: f32| Value::Single(a * b))
            }
            "SystemSingle.__op_Division__SystemSingle_SystemSingle__SystemSingle" => {
                binary_f32!(|a: f32, b: f32| Value::Single(a / b))
            }
            "SystemSingle.__op_Equality__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a == b))
            }
            "SystemSingle.__op_Inequality__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a != b))
            }
            "SystemSingle.__op_LessThan__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a < b))
            }
            "SystemSingle.__op_GreaterThan__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a > b))
            }
            "SystemSingle.__op_LessThanOrEqual__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a <= b))
            }
            "SystemSingle.__op_GreaterThanOrEqual__SystemSingle_SystemSingle__SystemBoolean" => {
                binary_f32!(|a: f32, b: f32| Value::Boolean(a >= b))
            }
            // ---- Boolean ----
            "SystemBoolean.__op_UnaryNegation__SystemBoolean__SystemBoolean" => {
                let args = self.pop_arguments(2)?;
                let a = self.heap[args[0]].as_bool()?;
                self.heap[args[1]] = Value::Boolean(!a);
                Ok(())
            }
            // ---- UInt32 (vtable/index math, delegate addresses) ----
            "SystemUInt32.__op_Addition__SystemUInt32_SystemUInt32__SystemUInt32" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_u32()?;
                let b = self.heap[args[1]].as_u32()?;
                self.heap[args[2]] = Value::UInt32(a.wrapping_add(b));
                Ok(())
            }
            "SystemUInt32.__op_Equality__SystemUInt32_SystemUInt32__SystemBoolean"
            | "SystemUInt32.__op_Inequality__SystemUInt32_SystemUInt32__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_u32()?;
                let b = self.heap[args[1]].as_u32()?;
                let wanted = (a == b) != signature.contains("op_Inequality");
                self.heap[args[2]] = Value::Boolean(wanted);
                Ok(())
            }
            "SystemBoolean.__op_LogicalAnd__SystemBoolean_SystemBoolean__SystemBoolean"
            | "SystemBoolean.__op_LogicalOr__SystemBoolean_SystemBoolean__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_bool()?;
                let b = self.heap[args[1]].as_bool()?;
                let value = if signature.contains("LogicalAnd") {
                    a && b
                } else {
                    a || b
                };
                self.heap[args[2]] = Value::Boolean(value);
                Ok(())
            }
            "SystemDouble.__op_Addition__SystemDouble_SystemDouble__SystemDouble"
            | "SystemDouble.__op_Subtraction__SystemDouble_SystemDouble__SystemDouble"
            | "SystemDouble.__op_Multiplication__SystemDouble_SystemDouble__SystemDouble"
            | "SystemDouble.__op_Division__SystemDouble_SystemDouble__SystemDouble" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_f64()?;
                let b = self.heap[args[1]].as_f64()?;
                let value = if signature.contains("Addition") {
                    a + b
                } else if signature.contains("Subtraction") {
                    a - b
                } else if signature.contains("Multiplication") {
                    a * b
                } else {
                    a / b
                };
                self.heap[args[2]] = Value::Double(value);
                Ok(())
            }
            "SystemDouble.__op_Equality__SystemDouble_SystemDouble__SystemBoolean"
            | "SystemDouble.__op_Inequality__SystemDouble_SystemDouble__SystemBoolean"
            | "SystemDouble.__op_LessThan__SystemDouble_SystemDouble__SystemBoolean"
            | "SystemDouble.__op_GreaterThan__SystemDouble_SystemDouble__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_f64()?;
                let b = self.heap[args[1]].as_f64()?;
                let value = if signature.contains("Equality") {
                    a == b
                } else if signature.contains("Inequality") {
                    a != b
                } else if signature.contains("LessThan") {
                    a < b
                } else {
                    a > b
                };
                self.heap[args[2]] = Value::Boolean(value);
                Ok(())
            }
            "SystemConvert.__ToDouble__SystemInt32__SystemDouble"
            | "SystemConvert.__ToDouble__SystemSingle__SystemDouble"
            | "SystemConvert.__ToDouble__SystemInt64__SystemDouble" => {
                let args = self.pop_arguments(2)?;
                let value = match &self.heap[args[0]] {
                    Value::Double(value) => *value,
                    Value::Single(value) => f64::from(*value),
                    Value::Int32(value) => f64::from(*value),
                    Value::Int64(value) => *value as f64,
                    other => {
                        return Err(EmulatorError::TypeError(format!(
                            "Convert.ToDouble of {other:?}"
                        )));
                    }
                };
                self.heap[args[1]] = Value::Double(value);
                Ok(())
            }
            // ---- Int64 and the numeric conversions nullable lifting uses ----
            "SystemInt64.__op_Addition__SystemInt64_SystemInt64__SystemInt64"
            | "SystemInt64.__op_Subtraction__SystemInt64_SystemInt64__SystemInt64"
            | "SystemInt64.__op_Multiplication__SystemInt64_SystemInt64__SystemInt64" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_i64()?;
                let b = self.heap[args[1]].as_i64()?;
                let value = if signature.contains("Addition") {
                    a.wrapping_add(b)
                } else if signature.contains("Subtraction") {
                    a.wrapping_sub(b)
                } else {
                    a.wrapping_mul(b)
                };
                self.heap[args[2]] = Value::Int64(value);
                Ok(())
            }
            "SystemInt64.__op_Equality__SystemInt64_SystemInt64__SystemBoolean"
            | "SystemInt64.__op_Inequality__SystemInt64_SystemInt64__SystemBoolean"
            | "SystemInt64.__op_LessThan__SystemInt64_SystemInt64__SystemBoolean"
            | "SystemInt64.__op_GreaterThan__SystemInt64_SystemInt64__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let a = self.heap[args[0]].as_i64()?;
                let b = self.heap[args[1]].as_i64()?;
                let value = if signature.contains("Equality") {
                    a == b
                } else if signature.contains("Inequality") {
                    a != b
                } else if signature.contains("LessThan") {
                    a < b
                } else {
                    a > b
                };
                self.heap[args[2]] = Value::Boolean(value);
                Ok(())
            }
            "SystemConvert.__ToInt64__SystemInt32__SystemInt64"
            | "SystemConvert.__ToInt64__SystemObject__SystemInt64" => {
                let args = self.pop_arguments(2)?;
                let value = match &self.heap[args[0]] {
                    Value::Int64(value) => *value,
                    other => i64::from(other.as_i32()?),
                };
                self.heap[args[1]] = Value::Int64(value);
                Ok(())
            }
            "SystemConvert.__ToInt32__SystemInt64__SystemInt32"
            | "SystemConvert.__ToInt32__SystemObject__SystemInt32" => {
                let args = self.pop_arguments(2)?;
                let value = match &self.heap[args[0]] {
                    Value::Int64(value) => *value as i32,
                    other => other.as_i32()?,
                };
                self.heap[args[1]] = Value::Int32(value);
                Ok(())
            }
            "SystemConvert.__ToUInt32__SystemObject__SystemUInt32" => {
                let args = self.pop_arguments(2)?;
                let value = match &self.heap[args[0]] {
                    Value::UInt32(value) => *value,
                    Value::Int32(value) => *value as u32,
                    other => {
                        return Err(EmulatorError::Exception(format!(
                            "Convert.ToUInt32 of {other:?}"
                        )));
                    }
                };
                self.heap[args[1]] = Value::UInt32(value);
                Ok(())
            }
            // ---- String ----
            "SystemString.__Concat__SystemString_SystemString__SystemString" => {
                let args = self.pop_arguments(3)?;
                let a = self.string_or_empty(args[0]);
                let b = self.string_or_empty(args[1]);
                self.heap[args[2]] = Value::Str(Rc::from(format!("{a}{b}")));
                Ok(())
            }
            "SystemString.__Concat__SystemString_SystemString_SystemString__SystemString" => {
                let args = self.pop_arguments(4)?;
                let joined = format!(
                    "{}{}{}",
                    self.string_or_empty(args[0]),
                    self.string_or_empty(args[1]),
                    self.string_or_empty(args[2])
                );
                self.heap[args[3]] = Value::Str(Rc::from(joined));
                Ok(())
            }
            "SystemString.__op_Equality__SystemString_SystemString__SystemBoolean"
            | "SystemString.__op_Inequality__SystemString_SystemString__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let equal = match (&self.heap[args[0]], &self.heap[args[1]]) {
                    (Value::Str(a), Value::Str(b)) => a == b,
                    (Value::Null, Value::Null) => true,
                    _ => false,
                };
                self.heap[args[2]] = Value::Boolean(equal == signature.contains("op_Equality"));
                Ok(())
            }
            "SystemChar.__IsLetter__SystemChar__SystemBoolean"
            | "SystemChar.__IsDigit__SystemChar__SystemBoolean"
            | "SystemChar.__IsWhiteSpace__SystemChar__SystemBoolean"
            | "SystemChar.__IsLetterOrDigit__SystemChar__SystemBoolean" => {
                let args = self.pop_arguments(2)?;
                let value = match &self.heap[args[0]] {
                    Value::Char(value) => *value,
                    other => {
                        return Err(EmulatorError::TypeError(format!(
                            "expected Char, found {other:?}"
                        )));
                    }
                };
                let result = match signature {
                    "SystemChar.__IsLetter__SystemChar__SystemBoolean" => value.is_alphabetic(),
                    "SystemChar.__IsDigit__SystemChar__SystemBoolean" => value.is_ascii_digit(),
                    "SystemChar.__IsWhiteSpace__SystemChar__SystemBoolean" => {
                        value.is_whitespace()
                    }
                    _ => value.is_alphanumeric(),
                };
                self.heap[args[1]] = Value::Boolean(result);
                Ok(())
            }
            "SystemString.__Substring__SystemInt32__SystemString"
            | "SystemString.__Substring__SystemInt32_SystemInt32__SystemString" => {
                let takes_length = signature.contains("SystemInt32_SystemInt32");
                let args = self.pop_arguments(if takes_length { 4 } else { 3 })?;
                let text: Vec<char> = self.heap[args[0]].as_str()?.chars().collect();
                let start = self.heap[args[1]].as_i32()? as usize;
                if start > text.len() {
                    return Err(EmulatorError::Exception("startIndex out of range".into()));
                }
                let length = if takes_length {
                    self.heap[args[2]].as_i32()? as usize
                } else {
                    text.len() - start
                };
                if start + length > text.len() {
                    return Err(EmulatorError::Exception("length out of range".into()));
                }
                let value: String = text[start..start + length].iter().collect();
                self.heap[*args.last().expect("an out slot")] = Value::Str(Rc::from(value));
                Ok(())
            }
            "SystemString.__get_Length__SystemInt32" => {
                let args = self.pop_arguments(2)?;
                let s = self.heap[args[0]].as_str()?;
                self.heap[args[1]] = Value::Int32(s.chars().count() as i32);
                Ok(())
            }
            "SystemString.__ToCharArray__SystemInt32_SystemInt32__SystemCharArray" => {
                let args = self.pop_arguments(4)?;
                let text = self.heap[args[0]].as_str()?;
                let start = self.heap[args[1]].as_i32()? as usize;
                let length = self.heap[args[2]].as_i32()? as usize;
                let chars: Vec<Value> = text
                    .chars()
                    .skip(start)
                    .take(length)
                    .map(Value::Char)
                    .collect();
                self.heap[args[3]] = Value::Array(Rc::new(RefCell::new(chars)));
                Ok(())
            }
            "SystemString.__ToCharArray__SystemCharArray" => {
                let args = self.pop_arguments(2)?;
                let chars: Vec<Value> = self.heap[args[0]]
                    .as_str()?
                    .chars()
                    .map(Value::Char)
                    .collect();
                self.heap[args[1]] = Value::Array(Rc::new(RefCell::new(chars)));
                Ok(())
            }
            "SystemInt32.__ToString__SystemString" => {
                let args = self.pop_arguments(2)?;
                let value = self.heap[args[0]].display();
                self.heap[args[1]] = Value::Str(Rc::from(value));
                Ok(())
            }
            // `Ref` marks an out/ref parameter: the extern writes through the
            // pushed address, exactly like an ordinary OUT result slot
            "SystemInt32.__TryParse__SystemString_SystemInt32Ref__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let text = self.string_or_empty(args[0]);
                let parsed = text.trim().parse::<i32>();
                self.heap[args[1]] = Value::Int32(*parsed.as_ref().unwrap_or(&0));
                self.heap[args[2]] = Value::Boolean(parsed.is_ok());
                Ok(())
            }
            "SystemObject.__ToString__SystemString"
            | "SystemBoolean.__ToString__SystemString"
            | "SystemChar.__ToString__SystemString"
            | "SystemConvert.__ToString__SystemObject__SystemString" => {
                let args = self.pop_arguments(2)?;
                let value = self.heap[args[0]].display();
                self.heap[args[1]] = Value::Str(Rc::from(value));
                Ok(())
            }
            "SystemInt32.__Parse__SystemString__SystemInt32" => {
                let args = self.pop_arguments(2)?;
                let text = self.string_or_empty(args[0]);
                match text.trim().parse::<i32>() {
                    Ok(value) => {
                        self.heap[args[1]] = Value::Int32(value);
                        Ok(())
                    }
                    Err(_) => Err(EmulatorError::Exception(format!(
                        "FormatException: The input string '{text}' was not in a correct format."
                    ))),
                }
            }
            // ---- Object identity / equality ----
            "SystemObject.__GetType__SystemType" => {
                let args = self.pop_arguments(2)?;
                let name = match &self.heap[args[0]] {
                    Value::Null => {
                        return Err(EmulatorError::TypeError("GetType on null".to_string()));
                    }
                    // .NET names, which is what a `typeof` constant carries
                    Value::Boolean(_) => "System.Boolean",
                    Value::Int32(_) => "System.Int32",
                    Value::Int64(_) => "System.Int64",
                    Value::UInt32(_) => "System.UInt32",
                    Value::Single(_) => "System.Single",
                    Value::Double(_) => "System.Double",
                    Value::Char(_) => "System.Char",
                    Value::Str(_) => "System.String",
                    // the emulator's arrays are untyped: every one reads as
                    // object[], which is what M# objects are
                    Value::Array(_) => "System.Object[]",
                    Value::Type(_) => "System.Type",
                    Value::SelfComponent(name) => &name.clone(),
                };
                self.heap[args[1]] = Value::Type(Rc::from(name));
                Ok(())
            }
            // no hierarchy without the real types: exact match, or `object`
            "SystemType.__IsInstanceOfType__SystemObject__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let wanted = match &self.heap[args[0]] {
                    Value::Type(name) => name.to_string(),
                    other => {
                        return Err(EmulatorError::TypeError(format!(
                            "IsInstanceOfType on {other:?}"
                        )));
                    }
                };
                let actual = match &self.heap[args[1]] {
                    Value::Null => None,
                    Value::Boolean(_) => Some("System.Boolean"),
                    Value::Int32(_) => Some("System.Int32"),
                    Value::Int64(_) => Some("System.Int64"),
                    Value::UInt32(_) => Some("System.UInt32"),
                    Value::Single(_) => Some("System.Single"),
                    Value::Double(_) => Some("System.Double"),
                    Value::Char(_) => Some("System.Char"),
                    Value::Str(_) => Some("System.String"),
                    Value::Array(_) => Some("System.Object[]"),
                    Value::Type(_) => Some("System.Type"),
                    Value::SelfComponent(_) => Some("<self>"),
                };
                let is = match actual {
                    None => false,
                    Some(actual) => wanted == "System.Object" || actual == wanted,
                };
                self.heap[args[2]] = Value::Boolean(is);
                Ok(())
            }
            "SystemType.__op_Equality__SystemType_SystemType__SystemBoolean"
            | "SystemType.__op_Inequality__SystemType_SystemType__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let equal = match (&self.heap[args[0]], &self.heap[args[1]]) {
                    (Value::Type(a), Value::Type(b)) => a == b,
                    (Value::Null, Value::Null) => true,
                    _ => false,
                };
                self.heap[args[2]] = Value::Boolean(equal == signature.contains("op_Equality"));
                Ok(())
            }
            "SystemObject.__GetHashCode__SystemInt32" => {
                let args = self.pop_arguments(2)?;
                let hash = match &self.heap[args[0]] {
                    Value::Null => 0,
                    Value::Boolean(value) => i32::from(*value),
                    Value::Int32(value) => *value,
                    Value::Int64(value) => (*value as i32) ^ ((*value >> 32) as i32),
                    Value::UInt32(value) => *value as i32,
                    Value::Char(value) => *value as i32,
                    Value::Str(text) => text.bytes().fold(17i32, |hash, byte| {
                        hash.wrapping_mul(31).wrapping_add(byte as i32)
                    }),
                    Value::Array(array) => Rc::as_ptr(array) as usize as i32,
                    other => {
                        return Err(EmulatorError::TypeError(format!(
                            "GetHashCode on {other:?}"
                        )));
                    }
                };
                self.heap[args[1]] = Value::Int32(hash);
                Ok(())
            }
            // the instance form has the same shape: receiver, other, out
            "SystemObject.__Equals__SystemObject__SystemBoolean"
            | "SystemObject.__Equals__SystemObject_SystemObject__SystemBoolean"
            | "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean"
            | "SystemObject.__op_Equality__SystemObject_SystemObject__SystemBoolean"
            | "SystemObject.__op_Inequality__SystemObject_SystemObject__SystemBoolean" => {
                let args = self.pop_arguments(3)?;
                let equal = match (&self.heap[args[0]], &self.heap[args[1]]) {
                    (Value::Null, Value::Null) => true,
                    (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
                    (Value::Int32(a), Value::Int32(b)) => a == b,
                    (Value::Boolean(a), Value::Boolean(b)) => a == b,
                    (Value::Str(a), Value::Str(b)) => a == b,
                    _ => false,
                };
                let wanted = equal != signature.contains("op_Inequality");
                self.heap[args[2]] = Value::Boolean(wanted);
                Ok(())
            }
            // ---- Arrays (any element type: the emulator's arrays are untyped) ----
            sig if is_array_signature(sig, "__ctor__SystemInt32__") => {
                let args = self.pop_arguments(2)?;
                let length = self.heap[args[0]].as_i32()?;
                // a typed array starts out holding default(T), as on Udon:
                // `new int[4]` is four zeros, not four nulls
                let default = match sig.split('.').next().unwrap_or_default() {
                    "SystemInt32Array" => Value::Int32(0),
                    "SystemInt64Array" => Value::Int64(0),
                    "SystemUInt32Array" => Value::UInt32(0),
                    "SystemSingleArray" => Value::Single(0.0),
                    "SystemDoubleArray" => Value::Double(0.0),
                    "SystemBooleanArray" => Value::Boolean(false),
                    "SystemCharArray" => Value::Char('\0'),
                    _ => Value::Null,
                };
                let elements = vec![default; length.max(0) as usize];
                self.heap[args[1]] = Value::Array(Rc::new(RefCell::new(elements)));
                Ok(())
            }
            sig if is_array_signature(sig, "__Get__SystemInt32__") => {
                let args = self.pop_arguments(3)?;
                let array = self.heap[args[0]].as_array()?;
                let index = self.heap[args[1]].as_i32()?;
                let elements = array.borrow();
                let value = elements
                    .get(index.max(0) as usize)
                    .filter(|_| index >= 0)
                    .cloned()
                    .ok_or(EmulatorError::IndexOutOfRange {
                        index,
                        length: elements.len(),
                    })?;
                drop(elements);
                self.heap[args[2]] = value;
                Ok(())
            }
            sig if is_array_signature(sig, "__Set__SystemInt32_") => {
                let args = self.pop_arguments(3)?;
                let array = self.heap[args[0]].as_array()?;
                let index = self.heap[args[1]].as_i32()?;
                let value = self.heap[args[2]].clone();
                let mut elements = array.borrow_mut();
                let length = elements.len();
                let slot = elements
                    .get_mut(index.max(0) as usize)
                    .filter(|_| index >= 0)
                    .ok_or(EmulatorError::IndexOutOfRange { index, length })?;
                *slot = value;
                Ok(())
            }
            sig if is_array_signature(sig, "__get_Length__") => {
                let args = self.pop_arguments(2)?;
                let array = self.heap[args[0]].as_array()?;
                let length = array.borrow().len();
                self.heap[args[1]] = Value::Int32(length as i32);
                Ok(())
            }
            "SystemArray.__Copy__SystemArray_SystemArray_SystemInt32__SystemVoid" => {
                let args = self.pop_arguments(3)?;
                let source = self.heap[args[0]].as_array()?;
                let destination = self.heap[args[1]].as_array()?;
                let count = self.heap[args[2]].as_i32()?.max(0) as usize;
                if Rc::ptr_eq(&source, &destination) {
                    return Ok(());
                }
                let source = source.borrow();
                let mut destination = destination.borrow_mut();
                for i in 0..count {
                    destination[i] = source[i].clone();
                }
                Ok(())
            }
            // ---- Debug ----
            "UnityEngineDebug.__LogError__SystemObject__SystemVoid" => {
                let args = self.pop_arguments(1)?;
                let text = self.heap[args[0]].display();
                self.log.push(text);
                Ok(())
            }
            "UnityEngineDebug.__Log__SystemObject__SystemVoid" => {
                let args = self.pop_arguments(1)?;
                let text = self.heap[args[0]].display();
                self.log.push(text);
                Ok(())
            }
            other => Err(EmulatorError::UnknownExtern(other.to_string())),
        }
    }

    fn string_or_empty(&self, address: usize) -> String {
        match &self.heap[address] {
            Value::Null => String::new(),
            value => value.display(),
        }
    }
}

/// Array externs exist per element type (`SystemInt32Array.__Get__…`,
/// `SystemObjectArray.__Get__…`, `UnityEngineVector3Array.__Get__…`). The
/// emulator treats them uniformly.
fn is_array_signature(signature: &str, middle: &str) -> bool {
    signature
        .split_once('.')
        .is_some_and(|(ty, rest)| ty.ends_with("Array") && rest.starts_with(middle))
}
