// MenSharp verification: the std's Http — `await Http.GetString(url)` /
// `GetJson<T>(url)` return a Result, completed by the SDK's string-loading
// events that the compiler exports into the program.
//
// Setup: a Cube "VerifyHttp" with this component; `url` set to the text
// "not a url" (the test does it). Play, click. No network: the SDK rejects
// an invalid URL on the spot through its error event, so both awaits
// complete within the click.
//
// Expected:
//   [verify-http] 1 requesting
//   [verify-http] 2 text: error 400: Invalid URL
//   [verify-http] 3 json: Err(400: Invalid URL)

using MenSharp;
using MenSharp.Net;
using UnityEngine;
using VRC.SDKBase;

public class HttpConfig
{
    public string title;
}

public class VerifyHttp : MenSharpBehaviour
{
    public VRCUrl url;

    public async void Interact()
    {
        Debug.Log("[verify-http] 1 requesting");
        Result<string, HttpError> text = await Http.GetString(url);
        text.Switch(
            value => Debug.Log("[verify-http] 2 text: " + value),
            error => Debug.Log("[verify-http] 2 text: error " + error));
        Result<HttpConfig, HttpError> json = await Http.GetJson<HttpConfig>(url);
        Debug.Log("[verify-http] 3 json: " + json);
    }
}
