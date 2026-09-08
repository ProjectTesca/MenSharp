// Bench corpus: pushes a rigidbody with an impulse that grows per use.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class PhysicsPush : MenSharpBehaviour
    {
        public float strength = 5f;
        public float growth = 1.1f;
        public Vector3 direction = Vector3.up;
        private Rigidbody body;
        private int pushes;

        private void Start()
        {
            body = GetComponent<Rigidbody>();
        }

        public void Interact()
        {
            if (body == null)
            {
                return;
            }
            float power = strength * Mathf.Pow(growth, pushes);
            body.AddForce(direction.normalized * power, ForceMode.Impulse);
            pushes++;
            if (pushes > 20)
            {
                pushes = 0;
            }
        }

        public float NextPower()
        {
            return strength * Mathf.Pow(growth, pushes);
        }
    }
}
