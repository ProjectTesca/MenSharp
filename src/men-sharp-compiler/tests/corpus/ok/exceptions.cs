// Exceptions: throw, catch by type with filters, rethrow, finally on every
// exit, custom exception classes, throw expressions.
using System;
using System.Collections.Generic;

namespace Corpus
{
    public class DoorLocked : InvalidOperationException
    {
        public int Code;
        public DoorLocked(int code) : base("locked " + code) { Code = code; }
    }

    public class Program
    {
        public static int total;
        public static string log = "";

        static void Deep(int n) { if (n == 0) throw new DoorLocked(42); Deep(n - 1); }

        static int WithFinally(bool boom)
        {
            try { if (boom) throw new Exception("boom"); return 1; }
            finally { log += "f"; }
        }

        static string Require(string s) => s ?? throw new ArgumentNullException(nameof(s));

        public static void Main()
        {
            try { Deep(3); }
            catch (DoorLocked e) when (e.Code == 41) { total = 1; }
            catch (DoorLocked e) { total = e.Code; }
            catch (Exception) { total = 2; }

            try
            {
                try { WithFinally(true); }
                catch (ArgumentException) { throw; }
                finally { log += "F"; }
            }
            catch (Exception e) { log += e.Message; }

            for (int i = 0; i < 3; i++)
            {
                try { if (i == 1) continue; if (i == 2) break; total += 10; }
                finally { total += 1; }
            }

            int[] arr = new int[2];
            try { arr[5] = 1; } catch (IndexOutOfRangeException) { total += 100; }
            var list = new List<int> { 1 };
            try { list[3] = 1; } catch (ArgumentOutOfRangeException) { total += 1000; }
            var map = new Dictionary<string, int>();
            try { int v = map["x"]; } catch (KeyNotFoundException) { total += 10000; }
            log += Require("ok") + WithFinally(false);
        }
    }
}
