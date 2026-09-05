// MenSharp verification: rectangular arrays (`int[,]`, `float[,,]`).
//
// Setup: a Cube "VerifyRectangular" with this component. Play, click.
//
// Expected:
//   [verify-rect] 1 basics: 1,4,6 / 6 2 3 rank 2
//   [verify-rect] 2 init: 21 / 12 / row-major 123456
//   [verify-rect] 3 cube: 24 / 0.5 / d
//   [verify-rect] 4 object: True / 5 / 100 1 / System.Int32[,]
//   [verify-rect] 5 bounds: caught IndexOutOfRangeException

using System;
using MenSharp;
using UnityEngine;

public class VerifyRectangular : MenSharpBehaviour
{
    private static int Sum(int[,] grid)
    {
        int total = 0;
        foreach (int value in grid) total += value;
        return total;
    }

    public void Interact()
    {
        int[,] a = new int[2, 3];
        a[0, 0] = 1;
        a[1, 2] = 6;
        a[0, 1] += 4;
        Debug.Log("[verify-rect] 1 basics: " + a[0, 0] + "," + a[0, 1] + "," + a[1, 2] + " / "
            + a.Length + " " + a.GetLength(0) + " " + a.GetLength(1) + " rank " + a.Rank);

        int[,] b = { { 1, 2, 3 }, { 4, 5, 6 } };
        var c = new int[,] { { 7, 8 }, { 9, 10 }, { 11, 12 } };
        int walked = 0;
        foreach (var v in b) walked = walked * 10 + v;
        Debug.Log("[verify-rect] 2 init: " + Sum(b) + " / " + c[2, 1] + " / row-major " + walked);

        float[,,] cube = new float[2, 3, 4];
        cube[1, 2, 3] = 0.5f;
        string[,] names = new string[2, 2] { { "a", "b" }, { "c", "d" } };
        Debug.Log("[verify-rect] 3 cube: " + cube.Length + " / " + cube[1, 2, 3] + " / " + names[1, 1]);

        object boxed = b;
        int[,] back = (int[,])boxed;
        var copy = (int[,])b.Clone();
        copy[0, 0] = 100;
        Debug.Log("[verify-rect] 4 object: " + (boxed is int[,]) + " / " + back[1, 1] + " / "
            + copy[0, 0] + " " + b[0, 0] + " / " + b);

        try
        {
            // flat index 3 exists in the data, but column 3 does not
            a[0, 3] = 1;
            Debug.Log("[verify-rect] 5 bounds: NOT caught");
        }
        catch (IndexOutOfRangeException)
        {
            Debug.Log("[verify-rect] 5 bounds: caught IndexOutOfRangeException");
        }
    }
}
