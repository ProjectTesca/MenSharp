// Bench corpus: counts players inside a trigger.
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

namespace MenSharpBench
{
    public class TriggerZone : MenSharpBehaviour
    {
        private int inside;
        private int entries;
        private string lastName = "";

        public void OnPlayerTriggerEnter(VRCPlayerApi player)
        {
            if (player == null)
            {
                return;
            }
            inside++;
            entries++;
            lastName = player.displayName;
            Debug.Log(lastName + " entered, " + inside + " inside");
        }

        public void OnPlayerTriggerExit(VRCPlayerApi player)
        {
            if (inside > 0)
            {
                inside--;
            }
        }

        public bool IsBusy()
        {
            return inside > 0;
        }

        public void Interact()
        {
            Debug.Log("zone: " + inside + " inside, " + entries + " entries, last " + lastName);
        }
    }
}
