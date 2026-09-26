// MenSharp mini-corlib: the behaviour base class, Unity flavour.
//
// The same type as MenSharpBehaviour.cs — exactly one of the two files is
// compiled, chosen by whether UnityEngine is among the references (see
// Compiler::corlib_sources_for). This one derives from MonoBehaviour, as the
// Unity-side MenSharpBehaviour really does, so that source written against
// that twin means the same thing to this compiler: `name`, `enabled`,
// `isActiveAndEnabled`, `GetInstanceID()`, `GetComponent<T>()`, `==` and
// `if (target)` are the engine's, on the behaviour itself and on any other
// behaviour it holds. At run time every behaviour value *is* its
// UdonBehaviour — a Component — so those members are the engine's externs
// on that value (the codegen's `with_engine_receiver` supplies the program's
// own UdonBehaviour when there is no receiver written). What Udon does not
// expose (`tag`, `hideFlags`) is reported as such.
//
// Udon has no `this`: `gameObject`, `transform` and `udonBehaviour` below
// are not calls but heap slots the SDK assembler initialises with its `this`
// literal, which the UdonBehaviour resolves to the GameObject it is attached
// to (and that GameObject's Transform) before the first event runs. Every
// auto-property declared *directly* on this class is lowered that way; on
// another behaviour the same names are the Component accessors instead.
//
// They are get-only on purpose: the Unity-side twin inherits them from
// Component, where they are read-only properties, and the same source file is
// compiled by both.

namespace MenSharp
{
    public class MenSharpBehaviour : UnityEngine.MonoBehaviour
    {
        public new UnityEngine.GameObject gameObject { get; }

        public new UnityEngine.Transform transform { get; }

        private System.Threading.CancellationTokenSource destroyTokenSource;

        public System.Threading.CancellationToken destroyCancellationToken
        {
            get
            {
                // Allocate the source lazily, as Unity does.
                if (destroyTokenSource == null)
                {
                    destroyTokenSource = new System.Threading.CancellationTokenSource();
                }
                return destroyTokenSource.Token;
            }
        }

        // Called before the user's OnDestroy by generated code.
        internal void __CancelDestroyToken()
        {
            if (destroyTokenSource != null)
            {
                destroyTokenSource.Cancel();
            }
        }

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
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2, parameter3);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2, parameter3, parameter4);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2, parameter3, parameter4, parameter5);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5, object parameter6)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2, parameter3, parameter4, parameter5, parameter6);
        }
        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5, object parameter6, object parameter7)
        {
            udonBehaviour.SendCustomNetworkEvent(target, eventName, parameter0, parameter1, parameter2, parameter3, parameter4, parameter5, parameter6, parameter7);
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

        // Udon has no TryGetComponent extern at all, so this is written out
        // in terms of GetComponent — which is exactly what it means.
        public bool TryGetComponent<T>(out T component)
        {
            component = GetComponent<T>();
            return component != null;
        }

        public void Destroy(UnityEngine.Object target)
        {
            UnityEngine.Object.Destroy(target);
        }

        public void Destroy(UnityEngine.Object target, float delay)
        {
            UnityEngine.Object.Destroy(target, delay);
        }

        // ------------------------------------------------------ VRC events
        // The events VRChat raises, as `virtual` methods — the shape
        // UdonSharpBehaviour gives them, so `public override void Interact()`
        // means the same thing here. Unity's own messages (Start, Update,
        // OnTriggerEnter, ...) are not declared: as on a MonoBehaviour they are
        // written from scratch, with any accessibility. Which names are events
        // is the SDK dump's business (see `udon_event_name`); this list is
        // its overridable subset, less the events whose argument types live
        // in assemblies the compiler is not handed (Economy, PhysBone,
        // Contact). The bodies do nothing: an event a class does not
        // override is simply not exported.
        public virtual void PostLateUpdate() { }
        public virtual void Interact() { }
        public virtual void OnAvatarChanged(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnAvatarEyeHeightChanged(VRC.SDKBase.VRCPlayerApi player, float prevEyeHeightAsMeters) { }
        public virtual void OnDrop() { }
        public virtual void OnOwnershipTransferred(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnMasterTransferred(VRC.SDKBase.VRCPlayerApi newMaster) { }
        public virtual void OnPickup() { }
        public virtual void OnPickupUseDown() { }
        public virtual void OnPickupUseUp() { }
        public virtual void OnPlayerJoined(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerLeft(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnSpawn() { }
        public virtual void OnStationEntered(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnStationExited(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnVideoEnd() { }
        public virtual void OnVideoError(VRC.SDK3.Components.Video.VideoError videoError) { }
        public virtual void OnVideoLoop() { }
        public virtual void OnVideoReady() { }
        public virtual void OnVideoStart() { }
        public virtual void OnPreSerialization() { }
        public virtual void OnDeserialization() { }
        public virtual void OnDeserialization(VRC.Udon.Common.DeserializationResult result) { }
        public virtual void OnPostSerialization(VRC.Udon.Common.SerializationResult result) { }
        public virtual void OnPlayerDataUpdated(VRC.SDKBase.VRCPlayerApi player, VRC.SDK3.Persistence.PlayerData.Info[] infos) { }
        public virtual void OnPlayerRestored(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerTriggerEnter(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerTriggerExit(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerTriggerStay(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerCollisionEnter(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerCollisionExit(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerCollisionStay(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerParticleCollision(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnControllerColliderHitPlayer(VRC.SDK3.ControllerColliderPlayerHit hit) { }
        public virtual void OnPlayerRespawn(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnImageLoadSuccess(VRC.SDK3.Image.IVRCImageDownload result) { }
        public virtual void OnImageLoadError(VRC.SDK3.Image.IVRCImageDownload result) { }
        public virtual void OnStringLoadSuccess(VRC.SDK3.StringLoading.IVRCStringDownload result) { }
        public virtual void OnStringLoadError(VRC.SDK3.StringLoading.IVRCStringDownload result) { }
        public virtual void OnPlayerSuspendChanged(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnDroneTriggerEnter(VRC.SDKBase.VRCDroneApi drone) { }
        public virtual void OnDroneTriggerExit(VRC.SDKBase.VRCDroneApi drone) { }
        public virtual void OnDroneTriggerStay(VRC.SDKBase.VRCDroneApi drone) { }
        public virtual bool OnOwnershipRequest(VRC.SDKBase.VRCPlayerApi requestingPlayer, VRC.SDKBase.VRCPlayerApi requestedOwner) { return true; }
        public virtual void MidiNoteOn(int channel, int number, int velocity) { }
        public virtual void MidiNoteOff(int channel, int number, int velocity) { }
        public virtual void MidiControlChange(int channel, int number, int value) { }
        public virtual void InputJump(bool value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputUse(bool value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputGrab(bool value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputDrop(bool value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputMoveHorizontal(float value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputMoveVertical(float value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputLookHorizontal(float value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void InputLookVertical(float value, VRC.Udon.Common.UdonInputEventArgs args) { }
        public virtual void OnInputMethodChanged(VRC.SDKBase.VRCInputMethod inputMethod) { }
        public virtual void OnLanguageChanged(string language) { }
        public virtual void OnAsyncGpuReadbackComplete(VRC.SDK3.Rendering.VRCAsyncGPUReadbackRequest request) { }
        public virtual void OnVRCCameraSettingsChanged(VRC.SDK3.Rendering.VRCCameraSettings cameraSettings) { }
        public virtual void OnVRCQualitySettingsChanged() { }
        public virtual void OnScreenUpdate(VRC.SDK3.Platform.ScreenUpdateData data) { }
        public virtual void OnVRCPlusMassGift(VRC.SDKBase.VRCPlayerApi gifter, int numGifts) { }
        public virtual void OnPersistenceUsageUpdated() { }
        public virtual void OnPlayerDataStorageExceeded(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerDataStorageWarning(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerObjectStorageExceeded(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnPlayerObjectStorageWarning(VRC.SDKBase.VRCPlayerApi player) { }
        public virtual void OnStationEntered() { }
        public virtual void OnStationExited() { }
        public virtual void OnOwnershipTransferred() { }

    }
}
