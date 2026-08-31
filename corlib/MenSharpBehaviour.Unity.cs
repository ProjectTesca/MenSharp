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

        // The program itself, as Udon's event-receiver interface. Everything a
        // behaviour can ask of itself — serialize now, raise this event — is an
        // extern on this, so declaring it here is what makes the methods below
        // ordinary code rather than compiler special cases.
        private VRC.Udon.Common.Interfaces.IUdonEventReceiver udonBehaviour { get; }

        public void RequestSerialization()
        {
            udonBehaviour.RequestSerialization();
        }

        public void SendCustomEvent(string eventName)
        {
            udonBehaviour.SendCustomEvent(eventName);
        }

        // VRChat replaces Unity's Instantiate with its own, and what it exposes
        // is a wrapper module rather than a type — there is no `VRCInstantiate`
        // class to call, only that extern. [UdonExtern] names it directly.
        //
        // The rest of Unity's overloads are missing on purpose: Udon has only
        // this one, so the others are written out here in terms of it, exactly
        // as they would be by hand.
        [UdonExtern("VRCInstantiate.__Instantiate__UnityEngineGameObject__UnityEngineGameObject")]
        public UnityEngine.GameObject Instantiate(UnityEngine.GameObject original)
        {
            return null;
        }

        public UnityEngine.GameObject Instantiate(
            UnityEngine.GameObject original,
            UnityEngine.Vector3 position,
            UnityEngine.Quaternion rotation)
        {
            UnityEngine.GameObject clone = Instantiate(original);
            clone.transform.SetPositionAndRotation(position, rotation);
            return clone;
        }

        public UnityEngine.GameObject Instantiate(
            UnityEngine.GameObject original,
            UnityEngine.Transform parent)
        {
            UnityEngine.GameObject clone = Instantiate(original);
            clone.transform.SetParent(parent, false);
            return clone;
        }

        public void Destroy(UnityEngine.Object target)
        {
            UnityEngine.Object.Destroy(target);
        }

        public void Destroy(UnityEngine.Object target, float delay)
        {
            UnityEngine.Object.Destroy(target, delay);
        }
    }
}
