// MenSharp mini-corlib: the behaviour base class, Unity flavour.
//
// The same type as MenSharpBehaviour.cs — exactly one of the two files is
// compiled, chosen by whether UnityEngine is among the references (see
// Compiler::corlib_sources_for). This one adds the members every Unity
// component inherits from UnityEngine.Component, so that source written
// against the Unity-side MenSharpBehaviour (which really does derive from
// MonoBehaviour) means the same thing to this compiler.
//
// Udon has no `this`: these are not calls but heap slots the SDK assembler
// initialises with its `this` literal, which the UdonBehaviour resolves to the
// GameObject it is attached to (and that GameObject's Transform) before the
// first event runs. Every member declared *directly* on this class is lowered
// that way, so adding one here is how the set grows.
//
// They are get-only on purpose: the Unity-side twin inherits them from
// Component, where they are read-only properties, and the same source file is
// compiled by both.

namespace MenSharp
{
    public class MenSharpBehaviour
    {
        public UnityEngine.GameObject gameObject { get; }

        public UnityEngine.Transform transform { get; }
    }
}
