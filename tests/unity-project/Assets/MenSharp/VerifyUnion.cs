// MenSharp verification: [Union] types dispatch on the runtime type, and a
// switch over one is checked for completeness at compile time.
//
// Setup: a Cube "VerifyUnion" with this component. Play, click.
//
// Expected:
//   [verify-union] 1 areas: 12 12 / C B
//   [verify-union] 2 events: joined bea / tick 7
//   [verify-union] 3 option: some 7 / none
//   [verify-union] 4 enum+bool: locked / no
//
// Compile-time check (uncomment `Forgot` to see the error): a switch that
// forgets `Box` is reported as
//   NonExhaustiveSwitch { subject: "UnionVerify.Shape", missing: ["UnionVerify.Box"] }

using System;
using MenSharp;
using UnityEngine;

namespace UnionVerify
{
    [Union] public abstract class Shape { }
    public sealed class Circle : Shape
    {
        public int R;
        public Circle(int r) { R = r; }
    }
    public sealed class Box : Shape
    {
        public int W, H;
        public Box(int w, int h) { W = w; H = h; }
        public void Deconstruct(out int w, out int h) { w = W; h = H; }
    }

    [Union] public interface IEvent { }
    public sealed class Joined : IEvent
    {
        public string Name;
        public Joined(string name) { Name = name; }
    }
    public sealed class Tick : IEvent
    {
        public int Frame;
        public Tick(int frame) { Frame = frame; }
    }

    [Union] public abstract class Option<T> { }
    public sealed class Some<T> : Option<T>
    {
        public T Value;
        public Some(T value) { Value = value; }
    }
    public sealed class None<T> : Option<T> { }

    public enum DoorState { Closed, Open, Locked }

    // The behaviour lives in the namespace too: VerifyCrash.cs declares a
    // top-level `Shape`, and a name in the enclosing namespace wins over a
    // `using` import (C#'s rule, and MenSharp's).
    public class VerifyUnion : MenSharpBehaviour
    {
        private static int Area(Shape s) => s switch
        {
            Circle c => c.R * c.R * 3,
            Box(var w, var h) => w * h,
        };

        private static string Kind(Shape s)
        {
            switch (s)
            {
                case Circle c: return "C";
                case Box b: return "B";
            }
            return "?";
        }

        private static string Describe(IEvent e) => e switch
        {
            Joined j => "joined " + j.Name,
            Tick t => "tick " + t.Frame,
        };

        private static string Show(Option<int> o) => o switch
        {
            Some<int> some => "some " + some.Value,
            None<int> none => "none",
        };

        private static string Name(DoorState door) => door switch
        {
            DoorState.Closed => "closed",
            DoorState.Open => "open",
            DoorState.Locked => "locked",
        };

        private static string YesNo(bool flag) => flag switch { true => "yes", false => "no" };

        // private static int Forgot(Shape s) => s switch { Circle c => 1 };

        public void Interact()
        {
            Shape[] shapes = { new Circle(2), new Box(3, 4) };
            string areas = "";
            string kinds = "";
            foreach (var s in shapes)
            {
                areas += Area(s) + " ";
                kinds += Kind(s) + " ";
            }
            Debug.Log("[verify-union] 1 areas: " + areas + "/ " + kinds);
            Debug.Log("[verify-union] 2 events: " + Describe(new Joined("bea")) + " / " + Describe(new Tick(7)));
            Debug.Log("[verify-union] 3 option: " + Show(new Some<int>(7)) + " / " + Show(new None<int>()));
            Debug.Log("[verify-union] 4 enum+bool: " + Name(DoorState.Locked) + " / " + YesNo(false));
        }
    }
}
