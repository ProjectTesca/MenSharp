---
icon: lucide/component
description: Behaviour どうしの呼び出し、エンジン API、例外
---

# Behaviour と例外

別の M# behaviour は文字列ではなく型で取得し、型付きのメソッドとして呼び出せます。エンジンのコンポーネントも同じ `GetComponent<T>` です。

```cs title="Ex7Behaviour.cs"
using MenSharp;
using UnityEngine;
using VRC.SDKBase;
using VRC.Udon.Common.Interfaces;

public class Door : MenSharpBehaviour
{
    [UdonSynced] public bool open;

    public void Toggle()
    {
        Networking.SetOwner(Networking.LocalPlayer, gameObject);
        open = !open;
        RequestSerialization();     // 同期フィールドを全員へ
        Apply();
    }

    public void OnDeserialization()
    {
        Apply();
    }

    private void Apply()
    {
        transform.localRotation = Quaternion.Euler(0f, open ? 90f : 0f, 0f);
    }
}

public class Ex7Behaviour : MenSharpBehaviour
{
    public GameObject prefab;

    public void Start()
    {
        // 別の M# behaviour を「型で」取得（文字列ではない）
        Door door = GetComponentInChildren<Door>();
        if (door != null)
        {
            door.Toggle();               // 本物の型付き呼び出し
            Debug.Log(door.open);
        }

        // エンジンのコンポーネントも同じ
        var body = GetComponent<Rigidbody>();
        if (body != null)
        {
            body.isKinematic = true;
        }

        SendCustomEvent("Later");
        SendCustomNetworkEvent(NetworkEventTarget.All, "Later");

        GameObject copy = Instantiate(prefab);
        Destroy(copy, 5f);

        try
        {
            Risky();
        }
        catch (System.InvalidOperationException error)
        {
            Debug.LogWarning(error.Message);   // 捕まえたので behaviour は生き延びる
        }
    }

    public void Later()
    {
        Debug.Log("later");
    }

    private void Risky()
    {
        throw new System.InvalidOperationException("nope");
    }
}
```

例外は `Exception` / `SystemException` / `InvalidOperationException` / `ArgumentException` / `ArgumentNullException` / `ArgumentOutOfRangeException` / `IndexOutOfRangeException` / `NullReferenceException` / `InvalidCastException` などが標準ライブラリにあります。`Message` / `InnerException` / `StackTrace` / `ToString()` が使えます。
