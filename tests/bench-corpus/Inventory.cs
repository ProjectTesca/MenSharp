// Bench corpus: a small inventory of named stacks.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class Inventory : MenSharpBehaviour
    {
        public int slots = 8;
        private string[] items;
        private int[] amounts;

        private void Start()
        {
            items = new string[slots];
            amounts = new int[slots];
        }

        private int Find(string item)
        {
            for (int i = 0; i < slots; i++)
            {
                if (items[i] == item)
                {
                    return i;
                }
            }
            return -1;
        }

        public bool Add(string item, int amount)
        {
            int at = Find(item);
            if (at < 0)
            {
                at = Find(null);
                if (at < 0)
                {
                    return false;
                }
                items[at] = item;
                amounts[at] = 0;
            }
            amounts[at] += amount;
            return true;
        }

        public bool Remove(string item, int amount)
        {
            int at = Find(item);
            if (at < 0 || amounts[at] < amount)
            {
                return false;
            }
            amounts[at] -= amount;
            if (amounts[at] == 0)
            {
                items[at] = null;
            }
            return true;
        }

        public string Summary()
        {
            string text = "";
            int used = 0;
            for (int i = 0; i < slots; i++)
            {
                if (items[i] != null)
                {
                    text += items[i] + " x" + amounts[i] + "; ";
                    used++;
                }
            }
            return used + "/" + slots + ": " + text;
        }

        public void Interact()
        {
            Add("gem", 3);
            Add("coin", 10);
            Remove("gem", 1);
            Debug.Log(Summary());
        }
    }
}
