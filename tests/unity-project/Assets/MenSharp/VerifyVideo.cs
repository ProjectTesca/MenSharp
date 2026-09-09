// MenSharp verification: members a VRC video player inherits from
// BaseVRCVideoPlayer.
//
// The SDK registers `LoadURL`, `Loop` and the rest once, on the abstract base
// class; the AVPro and Unity players only override them. A call through the
// derived type has to compile against the base's extern (as UdonSharp does),
// not against a derived extern that does not exist.
//
// Compile-only: the integration suite asserts this fixture yields a program
// asset; the runtime fixture list leaves it out, as a player has nothing to
// load in batchmode.

using MenSharp;
using UnityEngine;
using VRC.SDK3.Video.Components;
using VRC.SDK3.Video.Components.AVPro;
using VRC.SDKBase;

public class VerifyVideo : MenSharpBehaviour
{
    [SerializeField]
    private VRCAVProVideoPlayer avpvp;

    [SerializeField]
    private VRCUnityVideoPlayer uvp;

    [SerializeField]
    private VRCUrl url;

    public void Interact()
    {
        avpvp.LoadURL(url);
        uvp.LoadURL(url);
        uvp.Loop = !avpvp.Loop;
        Debug.Log($"[verify-video] playing: {avpvp.IsPlaying} {uvp.IsPlaying}");
    }
}
