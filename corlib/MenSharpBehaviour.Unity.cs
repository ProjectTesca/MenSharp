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

        // The program itself. Declared as UdonBehaviour rather than as the
        // interface its methods live on, because a `this` heap reference may
        // only be a GameObject, a Transform or an UdonBehaviour — Udon refuses
        // an interface-typed one and the whole program then fails to start.
        // The extern signatures still name the interface; the compiler
        // substitutes, exactly as UdonSharp does.
        private VRC.Udon.UdonBehaviour udonBehaviour { get; }

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

        // Unity lets you write these without a receiver, because they are
        // inherited from Component. Forwarding keeps the same source valid
        // here: `T` is monomorphized, so each instantiation reaches the extern
        // with its own typeof(T).
        public T GetComponent<T>()
        {
            return gameObject.GetComponent<T>();
        }

        public T GetComponentInChildren<T>()
        {
            return gameObject.GetComponentInChildren<T>();
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
