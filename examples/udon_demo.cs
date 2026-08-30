// MenSharp Udon demo: compile with
//
//   men-sharp --reference System.Private.CoreLib.dll \
//             --reference UnityEngine.CoreModule.dll \
//             --emit-udon Demo.Greeter --out greeter examples/udon_demo.cs
//
// then import greeter.uasm + greeter.meta.json with the Unity editor script
// in tools/unity/MenSharpProgramImporter.cs and drop the produced program
// asset onto an UdonBehaviour. `Start` runs on world load and logs greetings
// built with a generic List<T>, string interpolation and a class.

using System.Collections.Generic;
using UnityEngine;

namespace Demo
{
    public class Visitor
    {
        public string Name;
        public int Times;

        public Visitor(string name, int times)
        {
            Name = name;
            Times = times;
        }

        public string Greeting()
        {
            return $"hello {Name} (visit #{Times})";
        }
    }

    public class Greeter
    {
        public static int total;

        public static void Start()
        {
            var visitors = new List<Visitor>();
            visitors.Add(new Visitor("beatrice", 1));
            visitors.Add(new Visitor("claude", 2));

            total = 0;
            for (int i = 0; i < visitors.Count; i++)
            {
                Debug.Log(visitors[i].Greeting());
                total += visitors[i].Times;
            }
            Debug.Log($"total visits: {total}");
        }
    }
}
