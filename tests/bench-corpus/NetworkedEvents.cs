// Bench corpus: broadcasts custom network events and counts them.
using MenSharp;
using UnityEngine;
using VRC.SDKBase;
using VRC.Udon.Common.Interfaces;

namespace MenSharpBench
{
    public class NetworkedEvents : MenSharpBehaviour
    {
        private int pings;
        private int pongs;
        private float lastPingTime;

        public void Interact()
        {
            SendCustomNetworkEvent(NetworkEventTarget.All, "Ping");
        }

        public void Ping()
        {
            pings++;
            lastPingTime = Time.time;
            if (Networking.IsOwner(gameObject))
            {
                SendCustomNetworkEvent(NetworkEventTarget.Owner, "Pong");
            }
        }

        public void Pong()
        {
            pongs++;
            Debug.Log("pings " + pings + " pongs " + pongs);
        }

        public float SecondsSincePing()
        {
            return Time.time - lastPingTime;
        }
    }
}
