// MenSharp verification: VRCStringDownloader with `(IUdonEventReceiver)this`
// — the SDK calls the program back through its own UdonBehaviour.
//
// Setup: a Cube "VerifyStringLoad" with this component; `url` set to the
// text "not a url" (the test does it). Play, click. No network: an invalid
// URL is rejected by the SDK on the spot, through the error callback.
//
// Expected:
//   [verify-stringload] 1 requesting
//   [verify-stringload] 2 error 400: Invalid URL

using MenSharp;
using UnityEngine;
using VRC.SDK3.StringLoading;
using VRC.SDKBase;
using VRC.Udon.Common.Interfaces;

public class VerifyStringLoad : MenSharpBehaviour
{
    public VRCUrl url;

    public void Interact()
    {
        Debug.Log("[verify-stringload] 1 requesting");
        VRCStringDownloader.LoadUrl(url, (IUdonEventReceiver)this);
    }

    public void OnStringLoadSuccess(IVRCStringDownload result)
    {
        Debug.Log("[verify-stringload] 2 success: " + result.Result.Length + " chars");
    }

    public void OnStringLoadError(IVRCStringDownload result)
    {
        Debug.Log("[verify-stringload] 2 error " + result.ErrorCode + ": " + result.Error);
    }
}
