// The asset a marker file imports to (see MenSharpMarkerImporter). Empty on
// purpose: the marker works by existing. A file of its own because Unity
// resolves a ScriptableObject's script by the file name.
#if UNITY_EDITOR
using UnityEngine;

public sealed class MenSharpMarkerAsset : ScriptableObject
{
}
#endif
