// Bench corpus: vector math over a grid of points.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class VectorField : MenSharpBehaviour
    {
        public int size = 8;
        public Vector3 center;
        private Vector3[] points;

        private void Start()
        {
            points = new Vector3[size * size];
            for (int x = 0; x < size; x++)
            {
                for (int z = 0; z < size; z++)
                {
                    points[x * size + z] = new Vector3(x - size / 2, 0f, z - size / 2);
                }
            }
        }

        public void Interact()
        {
            float nearest = float.MaxValue;
            int nearestIndex = -1;
            Vector3 sum = Vector3.zero;
            for (int i = 0; i < points.Length; i++)
            {
                float distance = Vector3.Distance(points[i], center);
                if (distance < nearest)
                {
                    nearest = distance;
                    nearestIndex = i;
                }
                Vector3 away = points[i] - center;
                if (away.sqrMagnitude > 0.001f)
                {
                    sum += away.normalized;
                }
            }
            Vector3 forward = transform.forward;
            float facing = Vector3.Dot(sum.normalized, forward);
            Vector3 side = Vector3.Cross(forward, Vector3.up);
            Debug.Log("nearest " + nearestIndex + " at " + nearest + " facing " + facing + " side " + side);
        }
    }
}
