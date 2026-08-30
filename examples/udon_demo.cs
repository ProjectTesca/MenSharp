// MenSharp Udon demo, behaviour style: any class inheriting MenSharpBehaviour
// becomes one Udon program — no configuration. Its public methods are Udon
// events (Start runs on world load) and its public fields are the program's
// public variables, visible on the UdonBehaviour.
//
// In a Unity project with the MenSharp package: drop this under
// Assets/MenSharp/, press MenSharp > Compile All, and attach
// Assets/MenSharp/Programs/Greeter.asset to an UdonBehaviour.

using System.Collections.Generic;
using MenSharp;
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

    public class Greeter : MenSharpBehaviour
    {
        // a public variable: the inspector's value survives into Udon, so
        // setting it to 100 on the component makes the total come out at 103
        public int total;

        public void Start()
        {
            var visitors = new List<Visitor>();
            visitors.Add(new Visitor("beatrice", 1));
            visitors.Add(new Visitor("claude", 2));

            for (int i = 0; i < visitors.Count; i++)
            {
                Debug.Log(visitors[i].Greeting());
                total += visitors[i].Times;
            }
            Debug.Log($"total visits: {total}");
        }
    }
}
