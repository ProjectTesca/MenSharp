// MenSharp: `await`-able string loading — `MenSharp.Net.Http`.
//
//     Result<string, HttpError> text = await Http.GetString(url);
//     Result<Config, HttpError> config = await Http.GetJson<Config>(url);
//
// The SDK's `VRCStringDownloader` answers through two events on the
// behaviour that asked, `OnStringLoadSuccess` / `OnStringLoadError`. Here
// a request parks a `TaskCompletionSource` under its URL, and the events
// complete it; the compiler exports the two events into any program that
// uses `Http` (see the code generator's `add_std_event_handlers`), bound to
// `__OnStringLoadSuccess` / `__OnStringLoadError` below. A behaviour that
// declares either event itself takes it over — then it must forward the
// result to the handler here, or its awaits never finish.
//
// What the SDK enforces still holds: only `VRCUrl` values (set in the
// inspector or typed into a `VRCUrlInputField`), HTTP(S), one request per
// five seconds, no redirects, and the client's trusted-domain list unless
// the player allows untrusted URLs.
//
// Only present when the VRChat SDK is referenced. The Unity runtime
// assembly carries a twin with the same surface; it never runs there.

using System.Collections.Generic;
using System.Threading.Tasks;
using MenSharp.Internal;
using MenSharp.Json;
using VRC.SDK3.StringLoading;
using VRC.SDKBase;
using VRC.Udon.Common.Interfaces;

namespace MenSharp.Net
{
    /// Why a request failed: the HTTP status (or the SDK's own code — 400
    /// for a URL it rejects, 429 for too many requests) and its message; a
    /// `Code` of 0 is a failure after the download, such as JSON that did
    /// not fit.
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
        private sealed class Pending
        {
            public string Url;
            public TaskCompletionSource<Result<string, HttpError>> Source;
        }

        private static List<Pending> pending;

        /// The text at `url`, when it arrives.
        public static Task<Result<string, HttpError>> GetString(VRCUrl url)
        {
            if (pending == null)
            {
                pending = new List<Pending>();
            }
            var request = new Pending();
            request.Url = url.Get();
            request.Source = new TaskCompletionSource<Result<string, HttpError>>();
            pending.Add(request);
            VRCStringDownloader.LoadUrl(url, (IUdonEventReceiver)Programs.SelfBehaviour());
            return request.Source.Task;
        }

        /// The JSON at `url` read as a `T`, when it arrives; a parse error is
        /// an `HttpError` with `Code` 0.
        public static async Task<Result<T, HttpError>> GetJson<T>(VRCUrl url)
        {
            Result<string, HttpError> text = await GetString(url);
            if (text is Result<string, HttpError>.Err failed)
            {
                return new Result<T, HttpError>.Err(failed.Error);
            }
            Result<T, JsonError> parsed = Json.Json.Parse<T>(text.Value);
            if (parsed is Result<T, JsonError>.Err misfit)
            {
                return new Result<T, HttpError>.Err(new HttpError(0, misfit.Error.ToString()));
            }
            return new Result<T, HttpError>.Ok(parsed.Value);
        }

        /// What the program's `_onStringLoadSuccess` event runs.
        public static void __OnStringLoadSuccess(IVRCStringDownload result)
        {
            Complete(result.Url.Get(), new Result<string, HttpError>.Ok(result.Result));
        }

        /// What the program's `_onStringLoadError` event runs.
        public static void __OnStringLoadError(IVRCStringDownload result)
        {
            Complete(
                result.Url.Get(),
                new Result<string, HttpError>.Err(new HttpError(result.ErrorCode, result.Error)));
        }

        private static void Complete(string url, Result<string, HttpError> outcome)
        {
            if (pending == null)
            {
                return;
            }
            for (int i = 0; i < pending.Count; i++)
            {
                if (pending[i].Url == url)
                {
                    Pending request = pending[i];
                    pending.RemoveAt(i);
                    request.Source.TrySetResult(outcome);
                    return;
                }
            }
        }
    }
}
