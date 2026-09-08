// Bench corpus: ping-pongs between two points with easing.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Mover : MenSharpBehaviour
    {
        public Vector3 from;
        public Vector3 to = new Vector3(0f, 2f, 0f);
        public float seconds = 3f;
        public bool eased = true;
        private float clock;
        private bool forward = true;

        private void Start()
        {
            from = transform.localPosition;
        }

        private void Update()
        {
            clock += Time.deltaTime;
            if (clock >= seconds)
            {
                clock = 0f;
                forward = !forward;
            }
            float t = seconds > 0f ? clock / seconds : 1f;
            if (eased)
            {
                t = t * t * (3f - 2f * t);
            }
            Vector3 a = forward ? from : to;
            Vector3 b = forward ? to : from;
            transform.localPosition = Vector3.Lerp(a, b, t);
        }

        public float Progress()
        {
            return seconds > 0f ? clock / seconds : 1f;
        }
    }
}
