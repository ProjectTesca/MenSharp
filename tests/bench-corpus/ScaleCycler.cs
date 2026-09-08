// Bench corpus: cycles scale and computed colours through a palette.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class ScaleCycler : MenSharpBehaviour
    {
        public Color[] palette = new Color[] { Color.red, Color.green, Color.blue };
        public float secondsPerStep = 1f;
        private int step;
        private float clock;
        private Color current;

        private void Update()
        {
            clock += Time.deltaTime;
            float t = secondsPerStep > 0f ? clock / secondsPerStep : 1f;
            int next = (step + 1) % palette.Length;
            current = Color.Lerp(palette[step], palette[next], Mathf.Clamp01(t));
            float brightness = current.r * 0.3f + current.g * 0.6f + current.b * 0.1f;
            transform.localScale = Vector3.one * (0.5f + brightness);
            if (t >= 1f)
            {
                clock = 0f;
                step = next;
            }
        }

        public void Interact()
        {
            Debug.Log("colour " + current.r + "," + current.g + "," + current.b + " step " + step);
        }
    }
}
