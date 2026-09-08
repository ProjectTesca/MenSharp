// Bench corpus: everyday string handling.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class StringOps : MenSharpBehaviour
    {
        public string input = "The quick brown Fox jumps over the lazy Dog";
        private string report = "";

        public void Interact()
        {
            string[] words = input.Split(' ');
            int longest = 0;
            string longestWord = "";
            int capitalized = 0;
            for (int i = 0; i < words.Length; i++)
            {
                string word = words[i];
                if (word.Length > longest)
                {
                    longest = word.Length;
                    longestWord = word;
                }
                if (word.Length > 0 && word.Substring(0, 1) == word.Substring(0, 1).ToUpper())
                {
                    capitalized++;
                }
            }
            string reversed = "";
            for (int i = words.Length - 1; i >= 0; i--)
            {
                reversed += words[i];
                if (i > 0)
                {
                    reversed += " ";
                }
            }
            string shouting = input.ToUpper().Replace("THE", "A");
            int foxAt = input.IndexOf("Fox");
            bool hasDog = input.Contains("Dog");
            report = "words=" + words.Length + " longest=" + longestWord + " caps=" + capitalized
                + " fox@" + foxAt + " dog=" + hasDog + "\n" + reversed + "\n" + shouting;
            Debug.Log(report);
        }

        public string Report()
        {
            return report;
        }
    }
}
