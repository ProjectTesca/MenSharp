// MenSharp: `MenSharp.Net.Http`, as Unity's C# compiler sees it.
//
// The real one is the compiler's corlib (corlib/Http.cs): it parks a
// request under its URL and the SDK's string-loading events, exported into
// the program by the compiler, complete it. A MenSharpBehaviour's proxy
// never runs in Unity, so this twin only has to exist with the same
// surface; calling it says so.

using System;
using System.Threading.Tasks;
using VRC.SDK3.StringLoading;
using VRC.SDKBase;

namespace MenSharp.Net
{
    /// Why a request failed: the HTTP status (or the SDK's own code) and
    /// its message; a `Code` of 0 is a failure after the download, such as
    /// JSON that did not fit.
    public sealed class HttpError
    {
        public readonly int Code;
        public readonly string Message;

        public HttpError(int code, string message)
        {
            Code = code;
            Message = message;
        }

        public override string ToString()
        {
            return Code == 0 ? Message : Code + ": " + Message;
        }
    }

    public static class Http
    {
        /// The text at `url`, when it arrives.
        public static Task<Result<string, HttpError>> GetString(VRCUrl url)
        {
            throw new NotSupportedException("MenSharp.Net.Http runs only in a compiled MenSharp program");
        }

        /// The JSON at `url` read as a `T`, when it arrives.
        public static Task<Result<T, HttpError>> GetJson<T>(VRCUrl url)
        {
            throw new NotSupportedException("MenSharp.Net.Http runs only in a compiled MenSharp program");
        }

        public static void __OnStringLoadSuccess(IVRCStringDownload result)
        {
        }

        public static void __OnStringLoadError(IVRCStringDownload result)
        {
        }
    }
}
