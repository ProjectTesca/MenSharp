// Bench corpus: a fixed-size score table with insertion and sorting.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class ScoreBoard : MenSharpBehaviour
    {
        public int capacity = 10;
        private string[] names;
        private int[] scores;
        private int count;

        private void Start()
        {
            names = new string[capacity];
            scores = new int[capacity];
        }

        public void AddScore(string name, int score)
        {
            int at = -1;
            for (int i = 0; i < count; i++)
            {
                if (names[i] == name)
                {
                    at = i;
                    break;
                }
            }
            if (at < 0)
            {
                if (count < capacity)
                {
                    at = count;
                    count++;
                }
                else
                {
                    at = Lowest();
                    if (scores[at] >= score)
                    {
                        return;
                    }
                }
                names[at] = name;
                scores[at] = 0;
            }
            scores[at] += score;
            Sort();
        }

        private int Lowest()
        {
            int lowest = 0;
            for (int i = 1; i < count; i++)
            {
                if (scores[i] < scores[lowest])
                {
                    lowest = i;
                }
            }
            return lowest;
        }

        private void Sort()
        {
            for (int i = 0; i < count - 1; i++)
            {
                for (int j = 0; j < count - 1 - i; j++)
                {
                    if (scores[j] < scores[j + 1])
                    {
                        int s = scores[j];
                        scores[j] = scores[j + 1];
                        scores[j + 1] = s;
                        string n = names[j];
                        names[j] = names[j + 1];
                        names[j + 1] = n;
                    }
                }
            }
        }

        public string Render()
        {
            string text = "";
            for (int i = 0; i < count; i++)
            {
                text += (i + 1) + ". " + names[i] + " " + scores[i] + "\n";
            }
            return text;
        }

        public void Interact()
        {
            AddScore("player" + (count % 3), 10 + count);
            Debug.Log(Render());
        }
    }
}
