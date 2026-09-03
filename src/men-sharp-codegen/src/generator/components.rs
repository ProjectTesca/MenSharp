//! `GetComponent<Door>()` for a type argument that is a program.
//!
//! Udon's `GetComponent(typeof(...))` can only name engine types; a
//! behaviour of the user's (or an UdonSharp one) is an UdonBehaviour like
//! any other, told apart by what it carries: `__program_id` on a MenSharp
//! program, `__refl_typeid` / `__refl_typeids` on an UdonSharp one. So the
//! engine's `GetComponent` family, asked for a program type, is redirected
//! to the searches in `corlib/Programs.cs`, which ask every UdonBehaviour
//! on the object for its id. Those searches are ordinary M# code; what this
//! module lowers are the five intrinsics they rest on — the engine call for
//! the UdonBehaviours, `Is<T>` (which ids count as a `T` is a closed-world,
//! compile-time question) and `As<T>`.

use super::*;

const GET_COMPONENT_NAMES: [&str; 6] = [
    "GetComponent",
    "GetComponents",
    "GetComponentInChildren",
    "GetComponentsInChildren",
    "GetComponentInParent",
    "GetComponentsInParent",
];

const PROGRAMS_PATH: [&str; 3] = ["MenSharp", "Internal", "Programs"];

const BEHAVIOUR_TYPE_NAME: &str = "VRC.Udon.UdonBehaviour";

/// The value in a MenSharp program's heap slot 0: FNV-1a of its entry path.
pub(super) fn program_id_of(class_path: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in class_path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash & 0x7fff_ffff_ffff_ffff) as i64
}

/// UdonSharp's type id (`UdonSharpInternalUtility.GetTypeID`): the first
/// eight bytes, little-endian, of the SHA-256 of the type's full name.
pub(super) fn udonsharp_type_id(full_name: &str) -> i64 {
    let digest = sha256(full_name.as_bytes());
    i64::from_le_bytes(digest[..8].try_into().expect("eight bytes"))
}

impl<'a, 'ast> Generator<'a, 'ast> {
    /// The engine's `GetComponent<T>()` family with a program type as `T`:
    /// redirected to `MenSharp.Internal.Programs`, with the receiver's
    /// `transform` (an UdonBehaviour is found from any of its object's
    /// components). `None` when the call is not that.
    pub(super) fn try_get_component(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        receiver: &Option<(DataId, Type)>,
        values: &[DataId],
        span: Range<usize>,
    ) -> Option<Piece> {
        let MemberOrigin::External { member, .. } = &call.origin else {
            return None;
        };
        if call.is_static
            || call.type_arguments.len() != 1
            || !GET_COMPONENT_NAMES.contains(&member.name.as_str())
        {
            return None;
        }
        let target = self.substitute(&call.type_arguments[0], &ctx.key.bindings);
        if matches!(target, Type::Array { .. }) || !self.is_program_reference(&target) {
            return None;
        }
        let declaring = self.substitute(&call.declaring_type, &ctx.key.bindings);
        let owner = self.external_display_name(&declaring)?;
        if !matches!(
            owner.as_str(),
            "UnityEngine.GameObject" | "UnityEngine.Component"
        ) {
            return None;
        }
        let (receiver_slot, receiver_type) = receiver.clone()?;

        // the search wants the transform: the one component every object
        // has, and (unlike an `object`-typed slot) one Udon's externs accept
        let transform = self.temp("UnityEngineTransform");
        let receiver_owner = match self.external_display_name(&receiver_type).as_deref() {
            Some("UnityEngine.GameObject") => "UnityEngineGameObject",
            _ => "UnityEngineComponent",
        };
        self.call_extern(
            ctx,
            &format!("{receiver_owner}.__get_transform__UnityEngineTransform"),
            &[receiver_slot, transform],
            span.clone(),
        );

        let Some(programs) = self.find_symbol(&PROGRAMS_PATH) else {
            self.error(
                ctx,
                "internal: the corlib search `MenSharp.Internal.Programs` is missing",
                span,
            );
            return Some(Piece::Error);
        };
        let Some(&shim) = self
            .declarations
            .table
            .symbol(programs)
            .members_named(&member.name)
            .first()
        else {
            self.error(
                ctx,
                format!(
                    "internal: `MenSharp.Internal.Programs.{}` is missing",
                    member.name
                ),
                span,
            );
            return Some(Piece::Error);
        };
        let mut arguments = vec![transform];
        if member.name.ends_with("InChildren") || member.name.ends_with("InParent") {
            // `includeInactive`, defaulting to false as the engine does
            let include_inactive = match values.first() {
                Some(value) => *value,
                None => self.constant("SystemBoolean", "false", HeapInit::Boolean(false)),
            };
            arguments.push(include_inactive);
        }
        let Some(&type_parameter) = self.declarations.table.symbol(shim).type_parameters.first()
        else {
            return Some(Piece::Error);
        };
        let key = FunctionKey {
            symbol: shim,
            role: Role::Method,
            bindings: vec![(type_parameter, target)],
        };
        let return_type = self.substitute(&call.signature.return_type, &ctx.key.bindings);
        match self.call_function(ctx, &key, None, &arguments, &[], span) {
            Some(result) => Some(Piece::Value(result, return_type)),
            None => Some(Piece::Error),
        }
    }

    /// The five intrinsics of `MenSharp.Internal.Programs`, lowered in
    /// place. `None` for any other call.
    pub(super) fn try_program_intrinsic(
        &mut self,
        ctx: &mut Ctx<'ast>,
        call: &ResolvedCall,
        symbol: SymbolId,
        values: &[DataId],
        span: Range<usize>,
    ) -> Option<Piece> {
        let entry = self.declarations.table.symbol(symbol);
        let parent = entry.parent?;
        if self.display_path(parent) != PROGRAMS_PATH.join(".") {
            return None;
        }
        let name = entry.name;
        match name {
            "BehavioursOf" | "BehavioursInChildren" | "BehavioursInParent" => {
                let transform = self.temp("UnityEngineTransform");
                self.copy(*values.first()?, transform);
                let udon_behaviour = self.constant(
                    "SystemType",
                    BEHAVIOUR_TYPE_NAME,
                    HeapInit::TypeOf(BEHAVIOUR_TYPE_NAME.to_string()),
                );
                let out = self.temp("UnityEngineComponentArray");
                let (signature, mut pushed) = match name {
                    "BehavioursOf" => (
                        "UnityEngineComponent.__GetComponents__SystemType__UnityEngineComponentArray",
                        vec![transform, udon_behaviour],
                    ),
                    "BehavioursInChildren" => (
                        "UnityEngineComponent.__GetComponentsInChildren__SystemType_SystemBoolean__UnityEngineComponentArray",
                        vec![transform, udon_behaviour, *values.get(1)?],
                    ),
                    _ => (
                        "UnityEngineComponent.__GetComponentsInParent__SystemType_SystemBoolean__UnityEngineComponentArray",
                        vec![transform, udon_behaviour, *values.get(1)?],
                    ),
                };
                pushed.push(out);
                self.call_extern(ctx, signature, &pushed, span);
                let object_array = Type::Array {
                    element: Box::new(self.corlib_type("Object")),
                    rank: 1,
                };
                Some(Piece::Value(out, object_array))
            }
            "Is" => {
                let target = self.substitute(call.type_arguments.first()?, &ctx.key.bindings);
                let result = self.lower_is_program(ctx, *values.first()?, &target, span);
                Some(Piece::Value(result, self.corlib_type("Boolean")))
            }
            "As" => {
                let target = self.substitute(call.type_arguments.first()?, &ctx.key.bindings);
                let out = self.temp_for(&target);
                self.copy(*values.first()?, out);
                Some(Piece::Value(out, target))
            }
            _ => None,
        }
    }

    /// `Is<T>(behaviour)`: does the UdonBehaviour in `behaviour` (an
    /// `object` slot) run a program of type `T` — for a MenSharp `T`, one of
    /// the programs of `T` or its subclasses (all known: the world is
    /// closed); for an UdonSharp `T`, one whose `__refl_typeids` (or lone
    /// `__refl_typeid`) carries `T`'s id, which is how UdonSharp itself
    /// answers `GetComponent<T>` across inheritance.
    fn lower_is_program(
        &mut self,
        ctx: &mut Ctx<'ast>,
        behaviour: DataId,
        target: &Type,
        span: Range<usize>,
    ) -> DataId {
        let result = self.temp("SystemBoolean");
        let false_constant = self.constant("SystemBoolean", "false", HeapInit::Boolean(false));
        let true_constant = self.constant("SystemBoolean", "true", HeapInit::Boolean(true));
        self.copy(false_constant, result);
        let found = self.fresh_label("program_found");
        let done = self.fresh_label("program_done");

        // externs read the strongbox's type, not the value's: hand them the
        // behaviour in a slot of its own type
        let receiver = self.temp(BEHAVIOUR_HEAP_TYPE);
        self.copy(behaviour, receiver);
        let null = self.constant("SystemObject", "null", HeapInit::Null);

        let mensharp_class = match target {
            Type::Named {
                target: TypeTarget::Source(symbol),
                ..
            } if is_behaviour_class(self.declarations, self.signatures, *symbol) => Some(*symbol),
            _ => None,
        };
        if let Some(class) = mensharp_class {
            // the ids of every program that *is* a `T`
            let ids: Vec<i64> = behaviour_classes(self.declarations, self.signatures)
                .into_iter()
                .filter(|path| {
                    let segments: Vec<&str> = path.split('.').collect();
                    self.find_symbol(&segments)
                        .is_some_and(|symbol| self.derives_from(symbol, class))
                })
                .map(|path| program_id_of(&path))
                .collect();
            let value = self.read_program_int64(ctx, receiver, "__program_id", null, done, &span);
            for id in ids {
                self.jump_if_int64_equals(ctx, value, id, found, &span);
            }
        } else {
            let usharp_id = match target {
                Type::Named {
                    target: TypeTarget::Source(symbol),
                    ..
                } => Some(udonsharp_type_id(&self.display_path(*symbol))),
                // `UdonSharpBehaviour` itself: any UdonSharp program
                _ => None,
            };
            // the array of ids, when the program is part of a hierarchy...
            let single = self.fresh_label("program_single_id");
            self.jump_unless_program_has(ctx, receiver, "__refl_typeids", null, single, &span);
            let boxed = self.temp("SystemObject");
            let key = self.string_constant("__refl_typeids");
            self.call_extern(
                ctx,
                &format!(
                    "{BEHAVIOUR_EXTERN_TYPE}.__GetProgramVariable__SystemString__SystemObject"
                ),
                &[receiver, key, boxed],
                span.clone(),
            );
            let is_null = self.temp("SystemBoolean");
            self.call_extern(
                ctx,
                "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
                &[boxed, null, is_null],
                span.clone(),
            );
            self.jump_if(is_null, single);
            match usharp_id {
                Some(id) => {
                    let ids = self.temp("SystemInt64Array");
                    self.copy(boxed, ids);
                    self.jump_if_int64_array_contains(ctx, ids, id, found, &span);
                    self.program.code.push(Op::Jump(Target::Label(done)));
                }
                None => self.program.code.push(Op::Jump(Target::Label(found))),
            }
            // ...or the one id of a program without bases or subclasses
            self.program.code.push(Op::Label(single));
            let value = self.read_program_int64(ctx, receiver, "__refl_typeid", null, done, &span);
            match usharp_id {
                Some(id) => self.jump_if_int64_equals(ctx, value, id, found, &span),
                None => self.program.code.push(Op::Jump(Target::Label(found))),
            }
        }
        self.program.code.push(Op::Jump(Target::Label(done)));
        self.program.code.push(Op::Label(found));
        self.copy(true_constant, result);
        self.program.code.push(Op::Label(done));
        result
    }

    /// `GetProgramVariable(name)` as an `Int64`, jumping to `missing` when
    /// the program has no such variable (it is some other kind of program).
    fn read_program_int64(
        &mut self,
        ctx: &Ctx<'ast>,
        receiver: DataId,
        name: &str,
        null: DataId,
        missing: LabelId,
        span: &Range<usize>,
    ) -> DataId {
        self.jump_unless_program_has(ctx, receiver, name, null, missing, span);
        let boxed = self.temp("SystemObject");
        let key = self.string_constant(name);
        self.call_extern(
            ctx,
            &format!("{BEHAVIOUR_EXTERN_TYPE}.__GetProgramVariable__SystemString__SystemObject"),
            &[receiver, key, boxed],
            span.clone(),
        );
        let is_null = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[boxed, null, is_null],
            span.clone(),
        );
        self.jump_if(is_null, missing);
        let value = self.temp("SystemInt64");
        self.copy(boxed, value);
        value
    }

    /// Jumps to `missing` unless the program declares a variable `name`.
    /// `GetProgramVariable` on an absent name returns null *and* logs an
    /// error in the editor; `GetProgramVariableType` answers null quietly,
    /// so it is asked first — as UdonSharp's own search does.
    fn jump_unless_program_has(
        &mut self,
        ctx: &Ctx<'ast>,
        receiver: DataId,
        name: &str,
        null: DataId,
        missing: LabelId,
        span: &Range<usize>,
    ) {
        let key = self.string_constant(name);
        let variable_type = self.temp("SystemType");
        self.call_extern(
            ctx,
            &format!("{BEHAVIOUR_EXTERN_TYPE}.__GetProgramVariableType__SystemString__SystemType"),
            &[receiver, key, variable_type],
            span.clone(),
        );
        let absent = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemObject.__ReferenceEquals__SystemObject_SystemObject__SystemBoolean",
            &[variable_type, null, absent],
            span.clone(),
        );
        self.jump_if(absent, missing);
    }

    fn jump_if_int64_equals(
        &mut self,
        ctx: &Ctx<'ast>,
        value: DataId,
        id: i64,
        label: LabelId,
        span: &Range<usize>,
    ) {
        let constant = self.constant("SystemInt64", &id.to_string(), HeapInit::Int64(id));
        let equal = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemInt64.__op_Equality__SystemInt64_SystemInt64__SystemBoolean",
            &[value, constant, equal],
            span.clone(),
        );
        self.jump_if(equal, label);
    }

    /// A loop over an `Int64[]`, jumping to `label` at the first element
    /// equal to `id`.
    fn jump_if_int64_array_contains(
        &mut self,
        ctx: &Ctx<'ast>,
        ids: DataId,
        id: i64,
        label: LabelId,
        span: &Range<usize>,
    ) {
        let length = self.temp("SystemInt32");
        self.call_extern(
            ctx,
            "SystemInt64Array.__get_Length__SystemInt32",
            &[ids, length],
            span.clone(),
        );
        let index = self.temp("SystemInt32");
        let zero = self.constant("SystemInt32", "0", HeapInit::Int32(0));
        let one = self.constant("SystemInt32", "1", HeapInit::Int32(1));
        self.copy(zero, index);
        let head = self.fresh_label("ids_loop");
        let exit = self.fresh_label("ids_exit");
        self.program.code.push(Op::Label(head));
        let in_range = self.temp("SystemBoolean");
        self.call_extern(
            ctx,
            "SystemInt32.__op_LessThan__SystemInt32_SystemInt32__SystemBoolean",
            &[index, length, in_range],
            span.clone(),
        );
        self.program.code.push(Op::Push(in_range));
        self.program.code.push(Op::JumpIfFalse(Target::Label(exit)));
        let element = self.temp("SystemInt64");
        self.call_extern(
            ctx,
            "SystemInt64Array.__Get__SystemInt32__SystemInt64",
            &[ids, index, element],
            span.clone(),
        );
        self.jump_if_int64_equals(ctx, element, id, label, span);
        self.call_extern(
            ctx,
            "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
            &[index, one, index],
            span.clone(),
        );
        self.program.code.push(Op::Jump(Target::Label(head)));
        self.program.code.push(Op::Label(exit));
    }

    /// Is `class` `base`, or a class deriving from it (through source bases)?
    fn derives_from(&self, class: SymbolId, base: SymbolId) -> bool {
        let mut current = Some(class);
        for _ in 0..64 {
            let Some(symbol) = current else {
                return false;
            };
            if symbol == base {
                return true;
            }
            current = self
                .signatures
                .base_types
                .get(&symbol)
                .into_iter()
                .flatten()
                .find_map(|ty| match ty {
                    Type::Named {
                        target: TypeTarget::Source(id),
                        ..
                    } => Some(*id),
                    _ => None,
                });
        }
        false
    }

    /// The .NET full name of an external type, `None` for anything else.
    fn external_display_name(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Named {
                target: TypeTarget::External(id),
                ..
            } => Some(self.external.display_name(*id)),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ sha256

/// SHA-256 (FIPS 180-4), for UdonSharp's type ids. Small inputs only, so
/// simplicity over speed.
fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());

    for block in padded.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_digests() {
        let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // two blocks
        assert_eq!(
            hex(sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn udonsharp_type_ids_are_the_leading_int64_of_the_digest() {
        // SHA-256("abc") starts ba7816bf8f01cfea → little-endian Int64
        let expected = i64::from_le_bytes([0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea]);
        assert_eq!(udonsharp_type_id("abc"), expected);
    }
}
