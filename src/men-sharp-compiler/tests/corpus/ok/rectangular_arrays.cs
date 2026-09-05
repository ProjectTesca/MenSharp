// Rectangular arrays: creation, initializers, indexing, foreach and the
// System.Array members M# provides for them — csc and M# must both accept
// every line.
using System;

namespace Corpus
{
    public class Program
    {
        static int Sum(int[,] grid)
        {
            int total = 0;
            foreach (int value in grid)
            {
                total += value;
            }
            return total;
        }

        static T Corner<T>(T[,] grid) => grid[grid.GetLength(0) - 1, grid.GetLength(1) - 1];

        public static void Main()
        {
            int[,] a = new int[2, 3];
            a[0, 0] = 1;
            a[1, 2] += 6;
            int[,] b = { { 1, 2, 3 }, { 4, 5, 6 } };
            var c = new int[,] { { 7, 8 }, { 9, 10 }, { 11, 12 } };
            string[,] names = new string[2, 2] { { "a", "b" }, { "c", "d" } };
            float[,,] cube = new float[2, 3, 4];
            cube[1, 2, 3] = 0.5f;

            int text = a.Length + a.GetLength(0) + a.GetLength(1) + a.Rank
                + b.GetUpperBound(0) + b.GetLowerBound(1) + Sum(b) + c[2, 1] + Corner(c)
                + names[1, 0].Length + cube.Length + (int)cube[1, 2, 3];

            object boxed = b;
            if (boxed is int[,] grid)
            {
                text += grid[0, 0];
            }
            int[,] back = (int[,])boxed;
            var copy = (int[,])b.Clone();
            copy[0, 0] = 9;
            text += back[1, 1] + copy[0, 0] + b[0, 0];

            for (int i = 0; i < b.GetLength(0); i++)
            {
                for (int j = 0; j < b.GetLength(1); j++)
                {
                    text += b[i, j] * (i + j);
                }
            }
            Console.WriteLine(text);
        }
    }
}
