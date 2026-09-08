// `init` accessors (C# 9, which records use for their positional
// properties) compile against a marker type the C# compiler expects to find
// in the framework: System.Runtime.CompilerServices.IsExternalInit. .NET
// Standard 2.1, which Unity 2022.3 targets, does not have it, so it is
// declared here — the same polyfill every Unity project that uses records
// carries. It is public so that every assembly referencing MenSharp, the
// default Assembly-CSharp included, can use `init` and records.
//
// Nothing about it reaches Udon: MenSharp's own compiler never reads this
// file, and it exists only so Unity's compiler accepts the source.

namespace System.Runtime.CompilerServices
{
    public static class IsExternalInit
    {
    }
}
