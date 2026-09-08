// Bench corpus: a countdown timer formatted as mm:ss.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Timer : MenSharpBehaviour
    {
        public float duration = 90f;
        public bool loop;
        private float remaining;
        private bool running;
        private int lastWhole = -1;
        private string display = "";

        private void Start()
        {
            remaining = duration;
        }

        public void Interact()
        {
            running = !running;
            if (running && remaining <= 0f)
            {
                remaining = duration;
            }
        }

        private void Update()
        {
            if (!running)
            {
                return;
            }
            remaining -= Time.deltaTime;
            if (remaining <= 0f)
            {
                remaining = loop ? duration : 0f;
                running = loop;
                Debug.Log("timer finished");
            }
            int whole = Mathf.CeilToInt(remaining);
            if (whole != lastWhole)
            {
                lastWhole = whole;
                display = Format(whole);
            }
        }

        private string Format(int seconds)
        {
            int minutes = seconds / 60;
            int rest = seconds % 60;
            string m = minutes < 10 ? "0" + minutes : "" + minutes;
            string s = rest < 10 ? "0" + rest : "" + rest;
            return m + ":" + s;
        }

        public string Display()
        {
            return display;
        }
    }
}
