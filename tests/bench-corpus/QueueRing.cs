// Bench corpus: a ring buffer of ints with statistics.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class QueueRing : MenSharpBehaviour
    {
        public int capacity = 16;
        private int[] items;
        private int head;
        private int tail;
        private int size;
        private int pushed;

        private void Start()
        {
            items = new int[capacity];
        }

        public bool Enqueue(int value)
        {
            if (size == items.Length)
            {
                return false;
            }
            items[tail] = value;
            tail = (tail + 1) % items.Length;
            size++;
            pushed++;
            return true;
        }

        public int Dequeue()
        {
            if (size == 0)
            {
                return -1;
            }
            int value = items[head];
            head = (head + 1) % items.Length;
            size--;
            return value;
        }

        public int Sum()
        {
            int total = 0;
            for (int i = 0; i < size; i++)
            {
                total += items[(head + i) % items.Length];
            }
            return total;
        }

        public float Average()
        {
            return size == 0 ? 0f : (float)Sum() / size;
        }

        public void Interact()
        {
            if (!Enqueue(pushed * 7 % 23))
            {
                Dequeue();
                Enqueue(pushed * 7 % 23);
            }
            Debug.Log("queue size " + size + " sum " + Sum() + " avg " + Average());
        }
    }
}
