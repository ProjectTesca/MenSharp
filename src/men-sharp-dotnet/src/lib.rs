//! A reader for .NET assemblies (ECMA-335 CLI metadata).
//!
//! MenSharp code leans on types it never declares — `UnityEngine.Transform`,
//! `System.String`, the whole VRChat SDK — whose only definitions are the metadata
//! tables inside compiled managed dlls. This crate reads exactly the slice of that
//! format the compiler needs to *type-check against* an assembly: type definitions,
//! their members with full signatures, generic parameters, nesting, visibility and
//! constants. It reads nothing it does not need: no IL method bodies, no custom
//! attribute decoding, no resources, and it never writes.
//!
//! The format is platform-neutral (the PE wrapper notwithstanding), so this works
//! the same on any OS against any managed dll — Unity's, .NET's, or VRChat's.
//!
//! Like `men-sharp-parser`, the result borrows its input: strings in the model are
//! slices of the file bytes the caller keeps alive.
//!
//! ```no_run
//! use men_sharp_dotnet::DotNetAssembly;
//!
//! let bytes = std::fs::read("UnityEngine.CoreModule.dll").unwrap();
//! let assembly = DotNetAssembly::parse(&bytes).unwrap();
//!
//! let debug = assembly.find_type("UnityEngine", "Debug").unwrap();
//! for method in &assembly.type_definition(debug).methods {
//!     println!("{}", method.name);
//! }
//! ```

mod assembly;
mod error;
mod pe;
mod reader;
mod signature;
mod tables;

pub use assembly::{
    AssemblyReference, Constant, DotNetAssembly, EventDefinition, FieldDefinition,
    GenericParameter, MethodDefinition, ParameterDefinition, PropertyDefinition, TokenPath,
    TypeDefinition, TypeForwarder, TypeRefScope, TypeReference, Variance, split_arity,
};
pub use error::MetadataError;
pub use signature::{MethodSig, PropertySig, TypeSig, TypeToken};
