// Bench corpus: scrolls a message through a fixed window.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class TextTicker : MenSharpBehaviour
    {
        public string message = "Welcome to the world! Please enjoy your stay.   ";
        public int window = 12;
        public float secondsPerStep = 0.2f;
        private float clock;
        private int offset;
        private string visible = "";

        private void Update()
        {
            clock += Time.deltaTime;
            if (clock < secondsPerStep)
            {
                return;
            }
            clock -= secondsPerStep;
            offset = (offset + 1) % message.Length;
            visible = Slice(offset);
        }

        private string Slice(int start)
        {
            string text = "";
            for (int i = 0; i < window; i++)
            {
                int at = (start + i) % message.Length;
                text += message.Substring(at, 1);
            }
            return text;
        }

        public void Interact()
        {
            Debug.Log("[" + visible + "]");
        }
    }
}
