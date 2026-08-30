// MenSharp mini-corlib: the behaviour base class.
//
// Classes inheriting MenSharp.MenSharpBehaviour are the compilation's entry
// points: each becomes one Udon program, its public methods become Udon
// events (Start → _start, custom names → custom events), and its instance
// fields become the program's exported variables — editable in the Unity
// inspector.
//
// The Unity package ships a class with the same fully-qualified name that
// inherits MonoBehaviour, so the same source file also compiles under Unity
// (that is what powers IDE completion and, later, drag-and-drop onto
// GameObjects). This compiler-side twin deliberately declares no members yet:
// what is not declared cannot be miscompiled — Unity API access on `this`
// (transform, gameObject, ...) arrives together with its code generation.

namespace MenSharp
{
    public class MenSharpBehaviour
    {
    }
}
