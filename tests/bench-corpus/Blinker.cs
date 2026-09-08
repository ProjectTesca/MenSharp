// Bench corpus: blinks a renderer with a pattern of on/off durations.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Blinker : MenSharpBehaviour
    {
        public float[] pattern = new float[] { 0.5f, 0.5f, 0.1f, 0.1f, 0.1f, 1f };
        private Renderer target;
        private int index;
        private float left;
        private bool visible = true;

        private void Start()
        {
            target = GetComponent<Renderer>();
            if (pattern == null || pattern.Length == 0)
            {
                pattern = new float[] { 1f, 1f };
            }
            left = pattern[0];
        }

        private void Update()
        {
            left -= Time.deltaTime;
            while (left <= 0f)
            {
                index = (index + 1) % pattern.Length;
                left += pattern[index];
                visible = !visible;
                if (target != null)
                {
                    target.enabled = visible;
                }
            }
        }

        public float CycleLength()
        {
            float total = 0f;
            for (int i = 0; i < pattern.Length; i++)
            {
                total += pattern[i];
            }
            return total;
        }
    }
}
