// The base class every MenSharp behaviour inherits.
//
// This Unity-side twin makes your M# sources valid Unity C# (IDE completion,
// and later: drag the script straight onto a GameObject). The MenSharp
// compiler ships its own source for the same fully-qualified name, so what
// executes on Udon never depends on this class — it only has to exist.

using UnityEngine;

namespace MenSharp
{
    public class MenSharpBehaviour : MonoBehaviour
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
    }
}
