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
// (that is what powers IDE completion and drag-and-drop onto GameObjects).
//
// This is the Unity-free flavour, compiled when UnityEngine is not among the
// references — the compiler's own tests, mostly. It declares next to nothing:
// with no UnityEngine.GameObject to name, `gameObject` cannot be given a type,
// and what is not declared cannot be miscompiled. When UnityEngine *is*
// referenced, MenSharpBehaviour.Unity.cs is compiled instead of this file.

namespace MenSharp
{
    public class MenSharpBehaviour
    {
        // The VRC events that take no arguments, as `virtual` methods (see
        // MenSharpBehaviour.Unity.cs for the whole set and why): what a test
        // without UnityEngine can still `override`.
        public virtual void PostLateUpdate() { }
        public virtual void Interact() { }
        public virtual void OnDrop() { }
        public virtual void OnPickup() { }
        public virtual void OnPickupUseDown() { }
        public virtual void OnPickupUseUp() { }
        public virtual void OnSpawn() { }
        public virtual void OnVideoEnd() { }
        public virtual void OnVideoLoop() { }
        public virtual void OnVideoReady() { }
        public virtual void OnVideoStart() { }
        public virtual void OnPreSerialization() { }
        public virtual void OnDeserialization() { }
        public virtual void OnVRCQualitySettingsChanged() { }
        public virtual void OnPersistenceUsageUpdated() { }
        public virtual void OnStationEntered() { }
        public virtual void OnStationExited() { }
        public virtual void OnOwnershipTransferred() { }
    }
}
