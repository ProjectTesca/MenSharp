// Bench corpus: integer algorithms — fibonacci, primes, gcd.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class NumberCrunch : MenSharpBehaviour
    {
        public int limit = 200;

        private int Fibonacci(int n)
        {
            int a = 0;
            int b = 1;
            for (int i = 0; i < n; i++)
            {
                int next = a + b;
                a = b;
                b = next;
            }
            return a;
        }

        private int CountPrimes(int below)
        {
            bool[] composite = new bool[below + 1];
            int count = 0;
            for (int i = 2; i <= below; i++)
            {
                if (composite[i])
                {
                    continue;
                }
                count++;
                for (int j = i * 2; j <= below; j += i)
                {
                    composite[j] = true;
                }
            }
            return count;
        }

        private int Gcd(int a, int b)
        {
            while (b != 0)
            {
                int t = a % b;
                a = b;
                b = t;
            }
            return a;
        }

        public void Interact()
        {
            int fib = Fibonacci(30);
            int primes = CountPrimes(limit);
            int gcd = Gcd(fib, limit);
            sbyte small = 3 - 5;
            Debug.Log("fib30=" + fib + " primes<" + limit + "=" + primes + " gcd=" + gcd + " small=" + small);
        }
    }
}
