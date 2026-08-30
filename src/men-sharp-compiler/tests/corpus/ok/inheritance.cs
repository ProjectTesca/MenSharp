namespace Corpus
{
    public class Animal
    {
        public virtual string Speak() { return "..."; }
    }

    public class Dog : Animal
    {
        public override string Speak() { return "woof"; }
    }

    public class InheritanceUse
    {
        public int Run()
        {
            Animal a = new Dog();
            string sound = a.Speak();
            return sound.Length;
        }
    }
}
