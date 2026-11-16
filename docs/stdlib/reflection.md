---
icon: lucide/scan-search
description: MenSharp.Reflection の静的リフレクション
---

# 静的リフレクション

`MenSharp.Reflection` は、実行時に型情報を探すリフレクションではありません。visitor は型のフィールドを 1 つずつ、それぞれの静的型で受け取ります。`Visit<TField>` はコンパイル時にフィールドごとに実体化されます。

自前の属性は普通のクラスです。構築済みのインスタンスを `FieldInfo.Attribute<A>()` で取れます。`Reflect.IsArray<T>` などの判定はすべて定数で、`if` は取られる枝だけが残ります。どの枝にも入らない `T` があると、ビルドはその型名を挙げて失敗します。

```cs title="Ex4Reflect.cs"
using MenSharp;
using MenSharp.Reflection;
using UnityEngine;

// visitor は型のフィールドを 1 つずつ、それぞれの静的型で受け取る。
// Visit<TField> はコンパイル時にフィールドごとに実体化される。
public class Dumper : IFieldVisitor
{
    public string Text;

    public void Visit<TField>(FieldInfo field, ref TField value)
    {
        if (!field.IsPublic)
        {
            return;
        }
        Text = Text == null ? "" : Text + ", ";
        Text = Text + field.Name + "=" + value;
    }
}

// 自前の属性は普通のクラス。構築済みインスタンスが FieldInfo.Attribute<A>() で取れる
public class LabelAttribute : System.Attribute
{
    public readonly string Name;

    public LabelAttribute(string name)
    {
        Name = name;
    }
}

public struct Stats
{
    [Label("hp")] public int Health;
    public float Speed;
    public string Name;
}

public class Ex4Reflect : MenSharpBehaviour
{
    public void Start()
    {
        var stats = new Stats();
        stats.Health = 10;
        stats.Speed = 3.5f;
        stats.Name = "player";

        var dumper = new Dumper();
        Reflect.VisitFields(ref stats, dumper);
        Debug.Log(dumper.Text);       // Health=10, Speed=3.5, Name=player

        Debug.Log(Describe<int[]>());
        Debug.Log(Describe<Stats>());
    }

    // 以下の判定はすべて定数。if は取られる枝だけが残る
    private string Describe<T>()
    {
        if (Reflect.IsArray<T>())
        {
            return "an array";
        }
        else if (Reflect.IsList<T>())
        {
            return "a List";
        }
        else if (Reflect.IsNullable<T>())
        {
            return "a nullable";
        }
        else if (Reflect.IsEnum<T>())
        {
            return "an enum";
        }
        else if (Reflect.Is<T, int>() || Reflect.Is<T, string>())
        {
            return "a primitive";
        }
        else if (Reflect.IsObject<T>())
        {
            return "an object with fields";
        }
        else
        {
            // ここに到達する T があればビルドがその型名を挙げて落ちる
            Reflect.Unsupported<T>("Describe");
            return null;
        }
    }
}
```
