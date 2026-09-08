// Bench corpus: toggles a set of objects, and every child of a root, on interact.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Toggle : MenSharpBehaviour
    {
        public GameObject[] targets;
        public Transform childRoot;
        public bool startsOn = true;
        private bool isOn;
        private int toggles;

        private void Start()
        {
            isOn = startsOn;
            Apply();
        }

        public void Interact()
        {
            isOn = !isOn;
            toggles++;
            Apply();
        }

        private void Apply()
        {
            if (targets != null)
            {
                for (int i = 0; i < targets.Length; i++)
                {
                    GameObject target = targets[i];
                    if (target != null)
                    {
                        target.SetActive(isOn);
                    }
                }
            }
            if (childRoot != null)
            {
                int visible = 0;
                foreach (Transform child in childRoot)
                {
                    child.gameObject.SetActive(isOn);
                    if (isOn)
                    {
                        visible++;
                    }
                }
                Debug.Log("toggle " + toggles + ": " + visible + " children visible");
            }
        }

        public bool IsOn()
        {
            return isOn;
        }
    }
}
