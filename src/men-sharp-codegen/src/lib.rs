//! Lowering from checked M# programs to Udon assembly.
//!
//! [`externs`] embeds the whitelist the VRChat SDK exposes; [`generator`]
//! turns a checked compilation into a [`men_sharp_asm::Program`], validating
//! every emitted call against that whitelist.

pub mod externs;
pub mod generator;

pub use externs::{UdonNodes, mangle_dotnet_name};
pub use generator::{
    CodegenError, CodegenOutput, STATICS_HOLDER_PATH, STATICS_HOLDER_VARIABLE,
    STATICS_REFERENCE_VARIABLE, behaviour_classes, generate, generate_statics_holder,
};
