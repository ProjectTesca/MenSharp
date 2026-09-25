// The base class every MenSharp behaviour inherits.
//
// This Unity-side twin makes your M# sources valid Unity C# (IDE completion,
// and later: drag the script straight onto a GameObject). The MenSharp
// compiler ships its own source for the same fully-qualified name, so what
// executes on Udon never depends on this class — it only has to exist.

using UnityEngine;

namespace MenSharp
{
    public class MenSharpBehaviour : MonoBehaviour, VRC.Udon.Common.Interfaces.IUdonEventReceiver
    {
        // Stubs, so that source calling them is valid Unity C#. What runs is
        // the compiled Udon program, where each of these is an extern on the
        // UdonBehaviour itself; this component is stripped before play.

        /// Sends this behaviour's synced variables to everyone else. Only
        /// meaningful under `[UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]`.
        public void RequestSerialization()
        {
        }

        /// Raises an event on this behaviour by name — the same names its
        /// public methods export.
        public void SendCustomEvent(string eventName)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5, object parameter6)
        {
        }

        public void SendCustomNetworkEvent(VRC.Udon.Common.Interfaces.NetworkEventTarget target, string eventName, object parameter0, object parameter1, object parameter2, object parameter3, object parameter4, object parameter5, object parameter6, object parameter7)
        {
        }

        // The events VRChat raises, as `virtual` methods — the same set the
        // compiler's MenSharpBehaviour declares, so `public override void
        // Interact()` is valid Unity C# too. Unity's own messages are not
        // declared, as on any MonoBehaviour. Nothing runs: this component is
        // stripped before play.
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


        // ------------------------------------------- IUdonEventReceiver
        // What a compiled program is: the SDK's APIs take one where they
        // take an UdonBehaviour, so `this` must be one here too. Explicit
        // stubs, so nothing of this shows up beside your own members; the
        // public methods above already cover the rest of the interface.
        void VRC.Udon.Common.Interfaces.IUdonEventReceiver.SendCustomEventDelayedSeconds(string eventName, float delaySeconds, VRC.Udon.Common.Enums.EventTiming eventTiming) { }
        void VRC.Udon.Common.Interfaces.IUdonEventReceiver.SendCustomEventDelayedFrames(string eventName, int delayFrames, VRC.Udon.Common.Enums.EventTiming eventTiming) { }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.DisableInteractive { get { return false; } set { } }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.DisableEventProcessing { get { return false; } set { } }
        string VRC.Udon.Common.Interfaces.IUdonEventReceiver.InteractionText { get { return null; } set { } }
        void VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunProgram(string eventName) { }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunEvent(string eventName) { return false; }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunEvent<T0>(string eventName, (string symbolName, T0 value) parameter0) { return false; }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunEvent<T0, T1>(string eventName, (string symbolName, T0 value) parameter0, (string symbolName, T1 value) parameter1) { return false; }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunEvent<T0, T1, T2>(string eventName, (string symbolName, T0 value) parameter0, (string symbolName, T1 value) parameter1, (string symbolName, T2 value) parameter2) { return false; }
        bool VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunEvent(string eventName, params (string symbolName, object value)[] programVariables) { return false; }
        void VRC.Udon.Common.Interfaces.IUdonEventReceiver.RunInputEvent(string eventName, VRC.Udon.Common.UdonInputEventArgs args) { }
        System.Type VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.GetProgramVariableType(string symbolName) { return null; }
        T VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.GetProgramVariable<T>(string symbolName) { return default; }
        object VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.GetProgramVariable(string symbolName) { return null; }
        bool VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.TryGetProgramVariable<T>(string symbolName, out T value) { value = default; return false; }
        bool VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.TryGetProgramVariable(string symbolName, out object value) { value = null; return false; }
        void VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.SetProgramVariable<T>(string symbolName, T value) { }
        void VRC.Udon.Common.Interfaces.IUdonProgramVariableAccessTarget.SetProgramVariable(string symbolName, object value) { }

    }
}
