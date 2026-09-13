// MenSharp verification: a field shadowed across base and derived is two
// distinct heap slots — and each proxy-baked initializer must read its own
// class's field, not the most-derived one (issue: both read the derived URL).
//
// Setup: a Cube "VerifyShadowedUrl" with the derived component. Play, click.
//
// Expected:
//   [verify-shadow] base=https://example.com/base derived=https://example.com/derived

using MenSharp;
using UnityEngine;
using VRC.SDKBase;

public class ShadowUrlBase : MenSharpBehaviour
{
    private readonly VRCUrl url = new VRCUrl("https://example.com/base");

    protected string ReadBase() => url.Get();
}

public class VerifyShadowedUrl : ShadowUrlBase
{
    private readonly VRCUrl url = new VRCUrl("https://example.com/derived");

    public string baseUrl;
    public string derivedUrl;

    // a custom event so the test harness can invoke it by name (the same as
    // the user's Interact; the shadowing is independent of the method name)
    public void Run()
    {
        baseUrl = ReadBase();
        derivedUrl = url.Get();
        Debug.Log($"[verify-shadow] base={baseUrl} derived={derivedUrl}");
    }
}
