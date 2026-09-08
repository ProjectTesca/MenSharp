// Bench corpus: an elevator moving between floor heights, called by name.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Elevator : MenSharpBehaviour
    {
        public float[] floorHeights = new float[] { 0f, 3f, 6f, 9f };
        public float metersPerSecond = 2f;
        private int currentFloor;
        private int targetFloor;
        private int[] queue = new int[8];
        private int queued;

        public void GoUp()
        {
            Request(currentFloor + 1);
        }

        public void GoDown()
        {
            Request(currentFloor - 1);
        }

        public void Interact()
        {
            Request((currentFloor + 2) % floorHeights.Length);
        }

        private void Request(int floor)
        {
            if (floor < 0 || floor >= floorHeights.Length)
            {
                return;
            }
            for (int i = 0; i < queued; i++)
            {
                if (queue[i] == floor)
                {
                    return;
                }
            }
            if (queued < queue.Length)
            {
                queue[queued] = floor;
                queued++;
            }
        }

        private void Update()
        {
            float y = transform.localPosition.y;
            float goal = floorHeights[targetFloor];
            if (Mathf.Abs(y - goal) < 0.001f)
            {
                currentFloor = targetFloor;
                if (queued > 0)
                {
                    targetFloor = queue[0];
                    for (int i = 1; i < queued; i++)
                    {
                        queue[i - 1] = queue[i];
                    }
                    queued--;
                }
                return;
            }
            float next = Mathf.MoveTowards(y, goal, metersPerSecond * Time.deltaTime);
            Vector3 position = transform.localPosition;
            position.y = next;
            transform.localPosition = position;
        }

        public int Pending()
        {
            return queued;
        }
    }
}
