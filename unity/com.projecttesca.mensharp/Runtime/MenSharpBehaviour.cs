// The base class every MenSharp behaviour inherits.
//
// This Unity-side twin makes your M# sources valid Unity C# (IDE completion,
// and later: drag the script straight onto a GameObject). The MenSharp
// compiler ships its own source for the same fully-qualified name, so what
// executes on Udon never depends on this class — it only has to exist.

using UnityEngine;

namespace MenSharp
{
    public class MenSharpBehaviour : MonoBehaviour
    {
    }
}
