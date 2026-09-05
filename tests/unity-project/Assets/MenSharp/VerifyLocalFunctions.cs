// MenSharp verification: local functions — capture, recursion, delegates.
//
// Setup: a Cube "VerifyLocalFunctions" with this component. Play, click.
//
// Expected:
//   [verify-local] 1 basics: Twice(3)=6 above its declaration, Twice(4)=8, Step(1)=11, Step(x:0,by:5)=5
//   [verify-local] 2 recursion: Fact(5)=120, Even(10)=True, Odd(7)=True
//   [verify-local] 3 capture: total=10, word='hi!bye!', loop='<0><1><2>'
//   [verify-local] 4 ref/out: left=2 right=1, 48 -> high=4 low=8
//   [verify-local] 5 static/nesting: Pure(0)=1, Reads()=7, Outer()=240
//   [verify-local] 6 transitive: counter=3 (a lambda and a caller that never name it)
//   [verify-local] 7 delegate: group(6)=12, this: Doubled=6
//   [verify-local] 8 try: Risky(4)=25, Risky(0)=-1 caught 'zero'

using System;
using MenSharp;
using UnityEngine;

public class VerifyLocalFunctions : MenSharpBehaviour
{
    public int seed = 7;
    public int value = 3;

    private int Doubled()
    {
        int Twice() { return value * 2; }
        return Twice();
    }

    public void Interact()
    {
        // 1 — called above its own declaration, defaults and named arguments
        int first = Twice(3);
        int Twice(int x) { return x * 2; }
        int Step(int x, int by = 10) => x + by;
        Debug.Log($"[verify-local] 1 basics: Twice(3)={first} above its declaration, Twice(4)={Twice(4)}, Step(1)={Step(1)}, Step(x:0,by:5)={Step(by: 5, x: 0)}");

        // 2 — recursion and mutual recursion
        int Fact(int n) { return n <= 1 ? 1 : n * Fact(n - 1); }
        bool Even(int n) { return n == 0 ? true : Odd(n - 1); }
        bool Odd(int n) { return n == 0 ? false : Even(n - 1); }
        Debug.Log($"[verify-local] 2 recursion: Fact(5)={Fact(5)}, Even(10)={Even(10)}, Odd(7)={Odd(7)}");

        // 3 — captured variables are shared, not copied
        int total = 0;
        void Add(int by) { total += by; }
        Add(2);
        Add(3);
        total += 1;
        Add(4);

        string log = "";
        string word = "hi";
        void Shout() { log += word + "!"; }
        Shout();
        word = "bye";
        Shout();

        string loop = "";
        for (int i = 0; i < 3; i++)
        {
            string Tag() { return "<" + i + ">"; }
            loop += Tag();
        }
        Debug.Log($"[verify-local] 3 capture: total={total}, word='{log}', loop='{loop}'");

        // 4 — ref and out
        void Swap(ref int a, ref int b) { int t = a; a = b; b = t; }
        int left = 1;
        int right = 2;
        Swap(ref left, ref right);
        void Split(int v, out int high, out int low) { high = v / 10; low = v % 10; }
        Split(48, out int h, out int l);
        Debug.Log($"[verify-local] 4 ref/out: left={left} right={right}, 48 -> high={h} low={l}");

        // 5 — static, a field read, and one nested in another
        static int Pure(int x) { return x + 1; }
        int Reads() { return seed; }
        int outer = 100;
        int Outer()
        {
            int middle = 20;
            int Inner() { return outer + middle; }
            return Inner() + Inner();
        }
        Debug.Log($"[verify-local] 5 static/nesting: Pure(0)={Pure(0)}, Reads()={Reads()}, Outer()={Outer()}");

        // 6 — callers that never name `counter` still hand it over
        int counter = 0;
        void Bump() { counter++; }
        void BumpTwice() { Bump(); Bump(); }
        Action raise = () => Bump();
        BumpTwice();
        raise();
        Debug.Log($"[verify-local] 6 transitive: counter={counter} (a lambda and a caller that never name it)");

        // 7 — as a delegate, and `this` from an instance member
        Func<int, int> group = Twice;
        Debug.Log($"[verify-local] 7 delegate: group(6)={group(6)}, this: Doubled={Doubled()}");

        // 8 — its own try/catch
        int Risky(int x)
        {
            try
            {
                if (x == 0) throw new Exception("zero");
                return 100 / x;
            }
            catch (Exception e)
            {
                caught = e.Message;
                return -1;
            }
        }
        int ok = Risky(4);
        int bad = Risky(0);
        Debug.Log($"[verify-local] 8 try: Risky(4)={ok}, Risky(0)={bad} caught '{caught}'");
    }

    private string caught = "";
}
