// Bench corpus: tracks players joining and leaving.
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

namespace MenSharpBench
{
    public class PlayerJoinLog : MenSharpBehaviour
    {
        public int capacity = 80;
        private string[] present;
        private int count;
        private int joins;
        private int leaves;

        private void Start()
        {
            present = new string[capacity];
        }

        public void OnPlayerJoined(VRCPlayerApi player)
        {
            if (player == null)
            {
                return;
            }
            joins++;
            if (count < present.Length)
            {
                present[count] = player.displayName;
                count++;
            }
            Debug.Log("joined " + player.displayName + " (" + count + " here)");
        }

        public void OnPlayerLeft(VRCPlayerApi player)
        {
            if (player == null)
            {
                return;
            }
            leaves++;
            string name = player.displayName;
            for (int i = 0; i < count; i++)
            {
                if (present[i] == name)
                {
                    present[i] = present[count - 1];
                    present[count - 1] = null;
                    count--;
                    break;
                }
            }
        }

        public void Interact()
        {
            VRCPlayerApi local = Networking.LocalPlayer;
            string me = local != null ? local.displayName : "editor";
            Debug.Log(me + ": " + count + " present, " + joins + " joins, " + leaves + " leaves");
        }
    }
}
