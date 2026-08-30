using System.Collections.Generic;

namespace Corpus
{
    public class Box<T>
    {
        private readonly List<T> items = new List<T>();
        public void Add(T item) { items.Add(item); }
        public T First() { return items[0]; }
        public int Count => items.Count;
    }

    public class GenericUse
    {
        public string Run()
        {
            var box = new Box<string>();
            box.Add("hello");
            string first = box.First();
            return first + box.Count.ToString();
        }
    }
}
