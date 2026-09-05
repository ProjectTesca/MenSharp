// MenSharp verification: jagged arrays `int[][]`.
//
// Setup: a Cube "VerifyJagged" with this component. Play, click.
//
// Expected:
//   [verify-jagged] 1 rows: length=3, row 0 null before assignment=True, grid[0]={10,20}
//   [verify-jagged] 2 write: grid[1][0]=30, grid[1][1]=20, grid[0].Length=2
//   [verify-jagged] 3 written out: {1}{2,3}, widest=2, static table {1,2}{3}
//   [verify-jagged] 4 strings/deep: words[0][1]=b, deep[0][0][0]=7
//   [verify-jagged] 5 passed around: sum=6, foreach total=6

using MenSharp;
using UnityEngine;

public class VerifyJagged : MenSharpBehaviour
{
    public int rows = 3;

    private static int[][] Table = { new int[] { 1, 2 }, new int[] { 3 } };

    private int Widest(int[][] table)
    {
        int widest = 0;
        foreach (int[] row in table)
        {
            if (row != null && row.Length > widest) widest = row.Length;
        }
        return widest;
    }

    private string Join(int[][] table)
    {
        string text = "";
        foreach (int[] row in table)
        {
            text += "{";
            for (int i = 0; i < row.Length; i++)
            {
                if (i > 0) text += ",";
                text += row[i];
            }
            text += "}";
        }
        return text;
    }

    public void Interact()
    {
        int[][] grid = new int[rows][];
        bool emptyAtFirst = grid[0] == null;
        grid[0] = new int[] { 10, 20 };
        Debug.Log($"[verify-jagged] 1 rows: length={grid.Length}, row 0 null before assignment={emptyAtFirst}, grid[0]={{{grid[0][0]},{grid[0][1]}}}");

        grid[1] = new int[2];
        grid[1][0] = 30;
        grid[1][1] = grid[0][1];
        Debug.Log($"[verify-jagged] 2 write: grid[1][0]={grid[1][0]}, grid[1][1]={grid[1][1]}, grid[0].Length={grid[0].Length}");

        int[][] written = new int[][] { new int[] { 1 }, new int[] { 2, 3 } };
        Debug.Log($"[verify-jagged] 3 written out: {Join(written)}, widest={Widest(written)}, static table {Join(Table)}");

        string[][] words = { new string[] { "a", "b" } };
        int[][][] deep = new int[1][][];
        deep[0] = new int[1][];
        deep[0][0] = new int[] { 7 };
        Debug.Log($"[verify-jagged] 4 strings/deep: words[0][1]={words[0][1]}, deep[0][0][0]={deep[0][0][0]}");

        int sum = written[0][0] + written[1][0] + written[1][1];
        int total = 0;
        foreach (int[] row in written)
        {
            foreach (int value in row) total += value;
        }
        Debug.Log($"[verify-jagged] 5 passed around: sum={sum}, foreach total={total}");
    }
}
