// Bench corpus: an owner-controlled synced switch with a change callback.
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

namespace MenSharpBench
{
    [UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
    public class SyncedSwitch : MenSharpBehaviour
    {
        [UdonSynced] public bool isOn;
        [UdonSynced] public int flips;
        public GameObject lamp;
        private bool shown;

        public void Interact()
        {
            if (!Networking.IsOwner(gameObject))
            {
                Networking.SetOwner(Networking.LocalPlayer, gameObject);
            }
            isOn = !isOn;
            flips++;
            RequestSerialization();
            Show();
        }

        public void OnDeserialization()
        {
            Show();
        }

        private void Show()
        {
            if (shown == isOn)
            {
                return;
            }
            shown = isOn;
            if (lamp != null)
            {
                lamp.SetActive(isOn);
            }
            Debug.Log("switch " + (isOn ? "on" : "off") + " after " + flips + " flips");
        }
    }
}
