//! The Udon extern whitelist, embedded from the SDK dump.
//!
//! `data/udon/<unity version>.json` (see its README for provenance) enumerates
//! every node definition the VRChat SDK exposes. This module embeds those
//! dumps into the compiler binary — no downloads, no setup steps — parses them
//! lazily, and answers the two questions code generation asks constantly:
//!
//! - *does this extern signature exist?* (and how many parameters it pops)
//! - *is this type usable on the Udon heap?*
//!
//! Signature strings look like
//! `SystemString.__Concat__SystemString_SystemString__SystemString`; heap type
//! names like `SystemInt32` are .NET full names with `.`, `+` and generic
//! backticks squeezed out — [`mangle_dotnet_name`] implements that rule.

use std::collections::HashMap;
use std::sync::OnceLock;

/// One embedded dump per supported Unity editor version, newest last.
const DUMPS: &[(&str, &str)] = &[(
    "2022.3.22f1",
    include_str!("../../../data/udon/2022.3.22f1.json"),
)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    In,
    Out,
    InOut,
}

#[derive(Debug, Clone)]
pub struct ExternNode {
    pub parameters: Vec<ParameterKind>,
}

/// The whitelist for one Unity version.
pub struct UdonNodes {
    pub unity_version: String,
    externs: HashMap<String, ExternNode>,
    /// Mangled names of types the heap can declare (`Type_…` nodes).
    types: std::collections::HashSet<String>,
}

impl UdonNodes {
    /// The whitelist for the requested Unity version; an exact match if we
    /// have one, otherwise the newest embedded dump.
    pub fn for_unity_version(version: Option<&str>) -> &'static UdonNodes {
        static PARSED: OnceLock<Vec<UdonNodes>> = OnceLock::new();
        let all = PARSED.get_or_init(|| {
            DUMPS
                .iter()
                .map(|(version, json)| parse_dump(version, json))
                .collect()
        });
        version
            .and_then(|v| all.iter().find(|nodes| nodes.unity_version == v))
            .unwrap_or_else(|| all.last().expect("at least one dump is embedded"))
    }

    pub fn extern_node(&self, signature: &str) -> Option<&ExternNode> {
        self.externs.get(signature)
    }

    pub fn has_signature(&self, signature: &str) -> bool {
        self.externs.contains_key(signature)
    }

    pub fn has_type(&self, mangled: &str) -> bool {
        self.types.contains(mangled)
    }

    pub fn extern_count(&self) -> usize {
        self.externs.len()
    }
}

/// A *flat* .NET name (`Namespace.Outer+Nested`, `List` + backtick arity) →
/// its Udon spelling: separators vanish, a generic arity suffix vanishes.
/// Generic instantiations are mangled structurally by the code generator —
/// mangle the definition and each argument, then concatenate
/// (`SystemCollectionsGenericList` + `SystemInt32`).
pub fn mangle_dotnet_name(full: &str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut in_arity = false;
    for c in full.chars() {
        match c {
            '`' => in_arity = true,
            c if in_arity && c.is_ascii_digit() => {}
            c if c.is_ascii_alphanumeric() || c == '_' => {
                in_arity = false;
                out.push(c);
            }
            _ => in_arity = false,
        }
    }
    out
}

// ---------------------------------------------------------------- the parser
//
// The dump's schema is fixed (we generate it), so this is a minimal JSON
// reader specialised to it: it walks the `nodes` array and keeps `fullName`
// plus each parameter's `kind`. Anything unexpected is a hard panic — a bad
// embedded dump is a build problem, not a runtime condition.

fn parse_dump(version: &str, json: &str) -> UdonNodes {
    let mut reader = Reader {
        bytes: json.as_bytes(),
        at: 0,
    };
    let mut nodes = UdonNodes {
        unity_version: version.to_string(),
        externs: HashMap::new(),
        types: std::collections::HashSet::new(),
    };

    reader.expect(b'{');
    loop {
        let key = reader.string();
        reader.expect(b':');
        match key.as_str() {
            "unityVersion" => {
                nodes.unity_version = reader.string();
            }
            "nodes" => {
                reader.expect(b'[');
                if !reader.consume(b']') {
                    loop {
                        parse_node(&mut reader, &mut nodes);
                        if !reader.consume(b',') {
                            break;
                        }
                    }
                    reader.expect(b']');
                }
            }
            other => panic!("unexpected key {other:?} in udon dump"),
        }
        if !reader.consume(b',') {
            break;
        }
    }
    reader.expect(b'}');
    nodes
}

fn parse_node(reader: &mut Reader, nodes: &mut UdonNodes) {
    let mut full_name = String::new();
    let mut parameters = Vec::new();

    reader.expect(b'{');
    loop {
        let key = reader.string();
        reader.expect(b':');
        match key.as_str() {
            "fullName" => full_name = reader.string(),
            "relatedType" => {
                reader.string();
            }
            "parameters" => {
                reader.expect(b'[');
                if !reader.consume(b']') {
                    loop {
                        parameters.push(parse_parameter(reader));
                        if !reader.consume(b',') {
                            break;
                        }
                    }
                    reader.expect(b']');
                }
            }
            other => panic!("unexpected node key {other:?} in udon dump"),
        }
        if !reader.consume(b',') {
            break;
        }
    }
    reader.expect(b'}');

    // extern-shaped names carry a `__`; the rest are graph-editor nodes
    // (Branch, Event_…, Const_…, Type_…) that codegen never emits — but the
    // `Type_` ones do tell us which types the heap may declare.
    if full_name.contains(".__") {
        nodes.externs.insert(full_name, ExternNode { parameters });
    } else if let Some(type_name) = full_name.strip_prefix("Type_") {
        nodes.types.insert(type_name.to_string());
    }
}

fn parse_parameter(reader: &mut Reader) -> ParameterKind {
    let mut kind = ParameterKind::In;
    reader.expect(b'{');
    loop {
        let key = reader.string();
        reader.expect(b':');
        match key.as_str() {
            "kind" => {
                kind = match reader.string().as_str() {
                    "IN" => ParameterKind::In,
                    "OUT" => ParameterKind::Out,
                    "IN_OUT" => ParameterKind::InOut,
                    other => panic!("unexpected parameter kind {other:?}"),
                };
            }
            "name" | "type" => {
                // may be the literal `null`
                if !reader.consume_null() {
                    reader.string();
                }
            }
            other => panic!("unexpected parameter key {other:?}"),
        }
        if !reader.consume(b',') {
            break;
        }
    }
    reader.expect(b'}');
    kind
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn skip_whitespace(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) {
        self.skip_whitespace();
        assert_eq!(
            self.bytes.get(self.at),
            Some(&byte),
            "udon dump: expected {:?} at offset {}",
            byte as char,
            self.at
        );
        self.at += 1;
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.skip_whitespace();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn consume_null(&mut self) -> bool {
        self.skip_whitespace();
        if self.bytes[self.at..].starts_with(b"null") {
            self.at += 4;
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> String {
        self.expect(b'"');
        let mut out = String::new();
        loop {
            match self.bytes[self.at] {
                b'"' => {
                    self.at += 1;
                    return out;
                }
                b'\\' => {
                    self.at += 1;
                    let escaped = self.bytes[self.at];
                    self.at += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex =
                                std::str::from_utf8(&self.bytes[self.at..self.at + 4]).unwrap();
                            self.at += 4;
                            let code = u32::from_str_radix(hex, 16).unwrap();
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        other => panic!("unexpected escape \\{}", other as char),
                    }
                }
                _ => {
                    // copy a UTF-8 run without per-char decoding
                    let start = self.at;
                    while !matches!(self.bytes[self.at], b'"' | b'\\') {
                        self.at += 1;
                    }
                    out.push_str(std::str::from_utf8(&self.bytes[start..self.at]).unwrap());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_dump_parses_and_indexes() {
        let nodes = UdonNodes::for_unity_version(None);
        assert_eq!(nodes.unity_version, "2022.3.22f1");
        assert!(nodes.extern_count() > 30_000);

        // the signatures the code generator leans on hardest
        for signature in [
            "SystemInt32.__op_Addition__SystemInt32_SystemInt32__SystemInt32",
            "SystemObjectArray.__ctor__SystemInt32__SystemObjectArray",
            "SystemObjectArray.__Get__SystemInt32__SystemObject",
            "SystemObjectArray.__Set__SystemInt32_SystemObject__SystemVoid",
            "SystemArray.__Copy__SystemArray_SystemArray_SystemInt32__SystemVoid",
            "SystemString.__Concat__SystemString_SystemString__SystemString",
            "UnityEngineDebug.__Log__SystemObject__SystemVoid",
        ] {
            assert!(nodes.has_signature(signature), "missing {signature}");
        }

        // parameter shapes: instance + index + out
        let get = nodes
            .extern_node("SystemObjectArray.__Get__SystemInt32__SystemObject")
            .unwrap();
        assert_eq!(
            get.parameters,
            vec![ParameterKind::In, ParameterKind::In, ParameterKind::Out]
        );

        assert!(nodes.has_type("SystemInt32"));
        assert!(nodes.has_type("SystemObjectArray"));
    }

    #[test]
    fn version_selection_falls_back_to_newest() {
        let exact = UdonNodes::for_unity_version(Some("2022.3.22f1"));
        let fallback = UdonNodes::for_unity_version(Some("9999.9.9f9"));
        assert!(std::ptr::eq(exact, fallback));
    }

    #[test]
    fn dotnet_names_mangle_to_udon_names() {
        assert_eq!(mangle_dotnet_name("System.Int32"), "SystemInt32");
        assert_eq!(
            mangle_dotnet_name("UnityEngine.ParticleSystem+ExternalForcesModule"),
            "UnityEngineParticleSystemExternalForcesModule"
        );
        assert_eq!(
            mangle_dotnet_name("System.Collections.Generic.List`1"),
            "SystemCollectionsGenericList"
        );
        // generic instantiation = definition + arguments, composed structurally
        let list_of_int = format!(
            "{}{}",
            mangle_dotnet_name("System.Collections.Generic.List`1"),
            mangle_dotnet_name("System.Int32")
        );
        assert_eq!(list_of_int, "SystemCollectionsGenericListSystemInt32");
        assert!(UdonNodes::for_unity_version(None).has_type(&list_of_int));
    }
}
