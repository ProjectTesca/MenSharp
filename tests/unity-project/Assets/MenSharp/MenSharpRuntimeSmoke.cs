using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

// A deliberately small black-box test program. The Unity test runner invokes
// its exported events through the SDK's real UdonBehaviour and reads these
// public variables back from the Udon heap.
public class MenSharpRuntimeSmoke : MenSharpBehaviour
{
    public bool syncDone;
    public int syncResult;
    public string syncText;

    public bool asyncDone;
    public int asyncResult;

    // one count for every instance of this behaviour
    public static int shared;
    public int mine;

    // a `const string` read from an event — came up null once the SDK's
    // inspector had listed it as a public variable (issue)
    // a number cast to an engine enum has to arrive at an extern as the
    // boxed enum, not as the Int32 it was (issue: the Texture2D constructor
    // halted the behaviour)
    public int formatValue = 4; // TextureFormat.RGBA32
    public string formatName;
    public void MakeTexture()
    {
        TextureFormat format = (TextureFormat)formatValue;
        var texture = new Texture2D(2, 2, format, false);
        formatName = texture.format.ToString();
    }

    // operators on engine/corlib enums: computed on the int, boxed back —
    // and the result has to be what an extern accepts
    public int splitCount;
    public int flagsValue;
    public bool hasFlag;
    public string keyName;
    public void EnumOps()
    {
        StringSplitOptions options = StringSplitOptions.None | StringSplitOptions.RemoveEmptyEntries;
        splitCount = "a,,b".Split(new[] { ',' }, options).Length;
        flagsValue = (int)(options & ~StringSplitOptions.None);
        hasFlag = (options & StringSplitOptions.RemoveEmptyEntries) != 0;
        KeyCode key = KeyCode.A + 1;
        Input.GetKey(key);
        keyName = key.ToString();
    }

    const string greeting = "Hello";
    public string greeted;
    public void Greet()
    {
        greeted = $"Start: {greeting}";
    }

    public void RunShared()
    {
        shared++;
        mine = shared;
    }

    // a delegate of one instance, invoked by another: the invoker hands it
    // back to the program that made it
    public static event Action<int> OnPublished;
    public int toPublish;
    public int heard;

    public void Subscribe()
    {
        OnPublished += value => { heard = value * 10; };
    }

    public void Publish()
    {
        OnPublished?.Invoke(toPublish);
    }

    public void RunSync()
    {
        var values = new List<int> { 1, 2, 3, 4 };
        syncResult = values.Where(value => value % 2 == 0).Sum();
        syncText = $"sum={syncResult}";
        syncDone = true;
    }

    public async void RunAsync()
    {
        await Scheduler.NextFrame();
        asyncResult = 42;
        asyncDone = true;
    }
}
