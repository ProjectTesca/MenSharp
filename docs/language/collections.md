---
icon: lucide/list
description: List、Dictionary、LINQ、イテレータ
---

# コレクションと LINQ

`List<T>`、`Dictionary<K, V>`、LINQ、`yield return` によるイテレータが使えます。

```cs title="Ex6Collections.cs"
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public class Score
{
    public string name;
    public int points;
}

public class Ex6Collections : MenSharpBehaviour
{
    public void Start()
    {
        var scores = new List<Score>();
        scores.Add(NewScore("a", 30));
        scores.Add(NewScore("b", 10));
        scores.Add(NewScore("c", 20));

        // List<T>
        Debug.Log(scores.Count);
        scores.Sort((x, y) => y.points - x.points);
        Score best = scores.Find(s => s.points > 25);
        scores.RemoveAll(s => s.points < 15);
        foreach (Score score in scores)
        {
            Debug.Log(score.name + " " + score.points);
        }

        // Dictionary<K, V>
        var byName = new Dictionary<string, int>();
        byName["a"] = 30;
        byName.Add("b", 10);
        int points;
        if (byName.TryGetValue("a", out points))
        {
            Debug.Log(points);
        }
        foreach (KeyValuePair<string, int> pair in byName)
        {
            Debug.Log(pair.Key + " = " + pair.Value);
        }

        // LINQ
        string[] top = scores
            .Where(s => s.points >= 20)
            .OrderByDescending(s => s.points)
            .Select(s => s.name)
            .ToArray();
        Debug.Log(string.Join(", ", top) + " " + best.name);
        Debug.Log(scores.Sum(s => s.points) + " " + scores.Any(s => s.points > 100));

        // 自前のイテレータ
        foreach (int value in Evens(10))
        {
            Debug.Log(value);
        }
    }

    private Score NewScore(string name, int points)
    {
        var score = new Score();
        score.name = name;
        score.points = points;
        return score;
    }

    private IEnumerable<int> Evens(int limit)
    {
        for (int i = 0; i < limit; i += 2)
        {
            yield return i;
        }
    }
}
```

LINQ は次まで揃っています。

`Where` `Select` `SelectMany` `Take` `Skip` `TakeWhile` `SkipWhile` `Concat` `Append` `Prepend` `Zip` `Distinct` `Union` `Intersect` `Except` `OrderBy` / `OrderByDescending` `ThenBy` / `ThenByDescending` `Reverse` `GroupBy` `Range` `Repeat` `Empty` `Any` `All` `Contains` `SequenceEqual` `First` / `FirstOrDefault` `Last` / `LastOrDefault` `Single` / `SingleOrDefault` `ElementAt` / `ElementAtOrDefault` `Count` `LongCount` `Aggregate` `Sum` `Average` `Min` `Max` `ToArray` `ToList` `ToDictionary`
