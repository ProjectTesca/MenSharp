// MenSharp verification: external indexers and metadata conversion operators.
//
// Setup: a Cube "VerifyData" with this component. Play, click.
//
// Expected:
//   [verify-data] 1 list: count=3, [0]=1, [1]=two, [2]=3.5, after write [0]=9
//   [verify-data] 2 dictionary: count=2, a=1, b=hello, has a=True
//   [verify-data] 3 tokens: t=text (String), n=42 (Int), list token type=DataList
//   [verify-data] 4 vector: v[0]=1, v[1]=2, v[2]=3, after v[1]=9 -> (1.00, 9.00, 3.00)
//   [verify-data] 5 json: {"n":7} -> n=7, round trip ok=True

using UnityEngine;
using VRC.SDK3.Data;
using MenSharp;

public class VerifyData : MenSharpBehaviour
{
    public void Interact()
    {
        // 1 — DataList: implicit DataToken conversions, indexer read and write
        var list = new DataList();
        list.Add(1);
        list.Add("two");
        list.Add(3.5f);
        DataToken first = list[0];
        DataToken second = list[1];
        DataToken third = list[2];
        list[0] = 9;
        Debug.Log($"[verify-data] 1 list: count={list.Count}, [0]={first}, [1]={second}, [2]={third}, after write [0]={list[0]}");

        // 2 — DataDictionary: a string key, converted on the way in
        var map = new DataDictionary();
        map["a"] = 1;
        map["b"] = "hello";
        DataToken a = map["a"];
        DataToken b = map["b"];
        bool has = map.ContainsKey("a");
        Debug.Log($"[verify-data] 2 dictionary: count={map.Count}, a={a}, b={b}, has a={has}");

        // 3 — tokens keep their kind
        DataToken text = "text";
        DataToken number = 42;
        DataToken nested = list;
        Debug.Log($"[verify-data] 3 tokens: t={text} ({text.TokenType}), n={number.Double} ({number.TokenType}), list token type={nested.TokenType}");

        // 4 — a struct indexer from the engine
        Vector3 v = new Vector3(1, 2, 3);
        float x = v[0];
        float y = v[1];
        float z = v[2];
        v[1] = 9f;
        Debug.Log($"[verify-data] 4 vector: v[0]={x}, v[1]={y}, v[2]={z}, after v[1]=9 -> {v}");

        // 5 — the everyday reason all this matters: JSON
        string json = "{\"n\":7}";
        double parsed = -1;
        bool ok = false;
        if (VRCJson.TryDeserializeFromJson(json, out DataToken result))
        {
            parsed = result.DataDictionary["n"].Double;
            ok = VRCJson.TrySerializeToJson(result, JsonExportType.Minify, out DataToken back);
        }
        Debug.Log($"[verify-data] 5 json: {json} -> n={parsed}, round trip ok={ok}");
    }
}
