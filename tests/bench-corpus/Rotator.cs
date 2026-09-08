// Bench corpus: rotates itself, with a speed ramp and a pause.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Rotator : MenSharpBehaviour
    {
        public Vector3 degreesPerSecond = new Vector3(0f, 45f, 0f);
        public float rampSeconds = 2f;
        private float speed;
        private bool paused;

        private void Update()
        {
            float target = paused ? 0f : 1f;
            float rate = rampSeconds > 0f ? Time.deltaTime / rampSeconds : 1f;
            speed = Mathf.MoveTowards(speed, target, rate);
            if (speed > 0f)
            {
                transform.Rotate(degreesPerSecond * (speed * Time.deltaTime));
            }
        }

        public void Interact()
        {
            paused = !paused;
        }

        public float CurrentSpeed()
        {
            return speed;
        }
    }
}
