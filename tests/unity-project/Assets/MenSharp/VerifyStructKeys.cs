// MenSharp verification: user structs as Dictionary / HashSet keys — the
// synthesized field-wise Equals/GetHashCode over every field kind, value
// semantics of the stored key, generic structs, LINQ.
//
// Setup: a Cube "VerifyStructKeys" with this component. Play, click.
//
// Expected:
//   [verify-structkeys] 1 cells: count 2 found True b has True False removed True 1 walked 12b
//   [verify-structkeys] 2 copies: stored True True still True out 59
//   [verify-structkeys] 3 fields: count 4 t1 2 fresh 3 enum True float True string True null True nested True class True
//   [verify-structkeys] 4 generic: count 2 value 2 point 1 q False
//   [verify-structkeys] 5 set: count 2 dup False contains True removed True 1 linq 3 3 True 3 2

using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public enum TileKind { Wall, Floor }

public struct Cell
{
    public int x;
    public int y;
    public Cell(int x, int y) { this.x = x; this.y = y; }
}

public struct Tile
{
    public TileKind kind;
    public float height;
    public string name;
    public Cell at;
    public TileMarker marker;
}

public class TileMarker { public int id; }

public struct Pair<T>
{
    public T first;
    public T second;
}

// `record struct` would do here, but Unity 2022.3's own compiler is C# 9
// and rejects it (CS8773); M# accepts it — see the emulator test
public struct Point
{
    public int X;
    public int Y;
    public Point(int x, int y) { X = x; Y = y; }
}

public class VerifyStructKeys : MenSharpBehaviour
{
    public void Interact()
    {
        var grid = new Dictionary<Cell, string>();
        grid[new Cell(1, 2)] = "a";
        grid[new Cell(1, 2)] = "b";
        grid.Add(new Cell(2, 1), "c");
        int count = grid.Count;
        string got;
        bool found = grid.TryGetValue(new Cell(1, 2), out got);
        bool has = grid.ContainsKey(new Cell(2, 1));
        bool lacks = grid.ContainsKey(new Cell(3, 3));
        bool removed = grid.Remove(new Cell(2, 1));
        string walked = "";
        foreach (var (at, tag) in grid) { walked += at.x + "" + at.y + tag; }
        Debug.Log($"[verify-structkeys] 1 cells: count {count} found {found} {got} has {has} {lacks} removed {removed} {grid.Count} walked {walked}");

        Cell key = new Cell(5, 5);
        var byKey = new Dictionary<Cell, int>();
        byKey[key] = 1;
        key.x = 6;
        bool oldStillThere = byKey.ContainsKey(new Cell(5, 5));
        bool newAbsent = !byKey.ContainsKey(key);
        Cell first = new Cell(0, 0);
        foreach (var pair in byKey) { first = pair.Key; }
        first.y = 9;
        Debug.Log($"[verify-structkeys] 2 copies: stored {oldStillThere} {newAbsent} still {byKey.ContainsKey(new Cell(5, 5))} out {first.x}{first.y}");

        var shared = new TileMarker { id = 1 };
        var t1 = new Tile { kind = TileKind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
        var t2 = new Tile { kind = TileKind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
        var t3 = new Tile { kind = TileKind.Wall, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared };
        var t4 = new Tile { kind = TileKind.Floor, height = 0.5f, name = "n", at = new Cell(1, 1), marker = new TileMarker { id = 1 } };
        var t5 = new Tile { kind = TileKind.Floor, height = 0.5f, name = "n", at = new Cell(1, 2), marker = shared };
        var byTile = new Dictionary<Tile, int>();
        byTile[t1] = 1;
        byTile[t2] = 2;
        byTile[t3] = 3;
        byTile[t4] = 4;
        byTile[t5] = 5;
        int fresh = byTile[new Tile { kind = TileKind.Wall, height = 0.5f, name = "n", at = new Cell(1, 1), marker = shared }];
        bool enumEq = new Tile { kind = TileKind.Wall }.Equals(new Tile { kind = TileKind.Wall });
        bool floatEq = new Tile { height = 0.25f }.Equals(new Tile { height = 0.25f });
        bool stringEq = new Tile { name = "n" }.Equals(new Tile { name = "n" });
        bool nullEq = new Tile().Equals(new Tile());
        bool nestedEq = new Tile { at = new Cell(1, 1) }.Equals(new Tile { at = new Cell(1, 1) });
        bool classEq = new Tile { marker = shared }.Equals(new Tile { marker = shared }) && !new Tile { marker = shared }.Equals(new Tile { marker = new TileMarker { id = 1 } });
        Debug.Log($"[verify-structkeys] 3 fields: count {byTile.Count} t1 {byTile[t1]} fresh {fresh} enum {enumEq} float {floatEq} string {stringEq} null {nullEq} nested {nestedEq} class {classEq}");

        var pairs = new Dictionary<Pair<string>, int>();
        pairs[new Pair<string> { first = "a", second = "b" }] = 1;
        pairs[new Pair<string> { first = "a", second = "b" }] = 2;
        pairs[new Pair<string> { first = "b", second = "a" }] = 3;
        var points = new Dictionary<Point, string>();
        points[new Point(1, 1)] = "p";
        points[new Point(1, 1)] = "q";
        Debug.Log($"[verify-structkeys] 4 generic: count {pairs.Count} value {pairs[new Pair<string> { first = "a", second = "b" }]} point {points.Count} {points[new Point(1, 1)]} {points.ContainsKey(new Point(1, 2))}");

        var cells = new HashSet<Cell>();
        cells.Add(new Cell(1, 1));
        bool dup = cells.Add(new Cell(1, 1));
        cells.Add(new Cell(2, 2));
        bool contains = cells.Contains(new Cell(2, 2));
        bool gone = cells.Remove(new Cell(1, 1));
        var list = new List<Cell> { new Cell(1, 1), new Cell(2, 2), new Cell(1, 1), new Cell(3, 3) };
        Debug.Log($"[verify-structkeys] 5 set: count {cells.Count + 1} dup {dup} contains {contains} removed {gone} {cells.Count} linq {list.GroupBy(c => c).Count()} {list.Distinct().Count()} {list.Contains(new Cell(2, 2))} {list.IndexOf(new Cell(3, 3))} {list.Count(c => c.Equals(new Cell(1, 1)))}");
    }
}
