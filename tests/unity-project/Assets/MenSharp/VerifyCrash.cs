// MenSharp verification: an uncaught exception (here a failed cast, which
// throws InvalidCastException) must stop the program like an unhandled
// exception, at the cast, instead of reading the wrong object.
//
// Setup: make a GameObject "VerifyCrash" (a Cube, for the collider Interact
// needs) and add the VerifyCrash component. In play mode, click it.
//
// Expected in the Console, in this order:
//   [verify-crash] 1 before the cast
//   Unhandled exception: System.InvalidCastException: the object is not a `Square`
//      at VerifyCrash.Interact in Assets/MenSharp/VerifyCrash.cs:<line>:<col>   (our LogError)
//   the VM's own report that an exception occurred and the behaviour halted
// and *no* "[verify-crash] 2 ..." line. Clicking again does nothing more:
// the behaviour is halted.

using MenSharp;
using UnityEngine;

abstract class Shape { public abstract int Area(); }
class Circle : Shape { public int r = 2; public override int Area() { return 3 * r * r; } }
class Square : Shape { public int side = 4; public override int Area() { return side * side; } }

public class VerifyCrash : MenSharpBehaviour
{
    public void Interact()
    {
        Shape shape = new Circle();
        Debug.Log($"[verify-crash] 1 before the cast: shape is Square = {shape is Square}   (expect: False)");
        Square square = (Square)shape;   // InvalidCastException: halts here
        Debug.Log($"[verify-crash] 2 THIS MUST NOT APPEAR: side = {square.side}");
    }
}
