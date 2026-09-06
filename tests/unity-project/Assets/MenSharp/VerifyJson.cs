// MenSharp verification: the std's MenSharp.Json — Json.Parse<T> (a Result)
// and Json.Stringify<T> over static reflection.
//
// Setup: a Cube "VerifyJson" with this component. Play, click.
//
// Expected:
//   [verify-json] 1 scalars: title=Night Market, version=3, ratio=0.75, open=True, kind=1, note=(null)
//   [verify-json] 2 nested: owner=ann/7, tags=3 [a,b,c], scores=2 [10,20], players=2: bob/1 cy/2
//   [verify-json] 3 optional: limit=(null), bonus=5, missing=default, ignored=99, secret=hidden, alias=42
//   [verify-json] 4 round trip: {"title":"Night Market","version":3,"ratio":0.75,"open":true,"kind":1,"note":null,"owner":{"name":"ann","level":7},"tags":["a","b","c"],"scores":[10,20],"players":[{"name":"bob","level":1},{"name":"cy","level":2}],"limit":null,"bonus":5,"missing":0,"secret":"hidden","renamed":42,"where":{"x":1,"y":2}}
//   [verify-json] 5 struct: where=(1,2) -> {"x":1,"y":2}
//   [verify-json] 6 errors: $.owner.level: expected a number, found "seven" | $.players[1]: expected an object, found 5 | $: missing required key "title" | not JSON
//   [verify-json] 7 result: Err($.version: expected a number, found true)

using System.Collections.Generic;
using MenSharp;
using MenSharp.Json;
using UnityEngine;

namespace JsonVerify
{
    public enum Kind { Shop, Stage, Lobby }

    public struct Where
    {
        public int x;
        public int y;
    }

    public class Player
    {
        public string name;
        public int level;
    }

    public class Config
    {
        [JsonRequired] public string title;
        public int version;
        public float ratio;
        public bool open;
        public Kind kind;
        public string note;
        public Player owner;
        public string[] tags;
        public List<int> scores;
        public Player[] players;
        public int? limit;
        public int? bonus;
        public int missing;
        [JsonIgnore] public int ignored = 99;
        [JsonInclude] private string secret = "hidden";
        [JsonName("renamed")] public int alias;
        public Where where;

        public string Secret => secret;
    }

    public class VerifyJson : MenSharpBehaviour
    {
        private const string Text =
            "{\"title\":\"Night Market\",\"version\":3,\"ratio\":0.75,\"open\":true,\"kind\":1,\"note\":null,"
            + "\"owner\":{\"name\":\"ann\",\"level\":7},\"tags\":[\"a\",\"b\",\"c\"],\"scores\":[10,20],"
            + "\"players\":[{\"name\":\"bob\",\"level\":1},{\"name\":\"cy\",\"level\":2}],"
            + "\"limit\":null,\"bonus\":5,\"ignored\":1,\"secret\":\"hidden\",\"renamed\":42,\"where\":{\"x\":1,\"y\":2},\"extra\":true}";

        public void Interact()
        {
            Config config = Json.Parse<Config>(Text).Unwrap();
            Debug.Log($"[verify-json] 1 scalars: title={config.title}, version={config.version}, ratio={config.ratio}, open={config.open}, kind={(int)config.kind}, note={(config.note == null ? "(null)" : config.note)}");
            Debug.Log($"[verify-json] 2 nested: owner={config.owner.name}/{config.owner.level}, tags={config.tags.Length} [{string.Join(",", config.tags)}], scores={config.scores.Count} [{config.scores[0]},{config.scores[1]}], players={config.players.Length}: {config.players[0].name}/{config.players[0].level} {config.players[1].name}/{config.players[1].level}");
            Debug.Log($"[verify-json] 3 optional: limit={(config.limit == null ? "(null)" : config.limit.ToString())}, bonus={config.bonus}, missing={(config.missing == 0 ? "default" : "set")}, ignored={config.ignored}, secret={config.Secret}, alias={config.alias}");
            Debug.Log("[verify-json] 4 round trip: " + Json.Stringify(config));
            Debug.Log($"[verify-json] 5 struct: where=({config.where.x},{config.where.y}) -> {Json.Stringify(config.where)}");

            string e1 = ErrorOf("{\"title\":\"t\",\"owner\":{\"name\":\"ann\",\"level\":\"seven\"}}");
            string e2 = ErrorOf("{\"title\":\"t\",\"players\":[{\"name\":\"bob\"},5]}");
            string e3 = ErrorOf("{\"version\":1}");
            string e4 = ErrorOf("{not json");
            Debug.Log($"[verify-json] 6 errors: {e1} | {e2} | {e3} | {e4}");

            Debug.Log("[verify-json] 7 result: " + Json.Parse<Config>("{\"title\":\"t\",\"version\":true}"));
        }

        private static string ErrorOf(string text)
        {
            Result<Config, JsonError> parsed = Json.Parse<Config>(text);
            if (parsed.IsOk)
            {
                return "no error";
            }
            return parsed.Error.Message.StartsWith("not JSON") ? "not JSON" : parsed.Error.ToString();
        }
    }
}
