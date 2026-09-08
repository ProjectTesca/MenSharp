// Bench corpus: a synced click counter. Compiles under UdonSharp and MenSharp
// alike (tools/prepare-bench.sh derives both variants from this file).
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

namespace MenSharpBench
{
    [UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
    public class Counter : MenSharpBehaviour
    {
        [UdonSynced] public int count;
        public int step = 1;
        public int limit = 100;
        public string label = "clicks";
        private int localClicks;
        private string lastMessage = "";

        public void Interact()
        {
            if (!Networking.IsOwner(gameObject))
            {
                Networking.SetOwner(Networking.LocalPlayer, gameObject);
            }
            count = count + step;
            if (count > limit)
            {
                count = 0;
            }
            localClicks++;
            RequestSerialization();
            Refresh();
        }

        public void OnDeserialization()
        {
            Refresh();
        }

        public void Reset()
        {
            count = 0;
            localClicks = 0;
            Refresh();
        }

        private void Refresh()
        {
            lastMessage = label + ": " + count + " (" + localClicks + " local)";
            Debug.Log(lastMessage);
        }

        public int Remaining()
        {
            int remaining = limit - count;
            if (remaining < 0)
            {
                remaining = 0;
            }
            return remaining;
        }
    }
}
