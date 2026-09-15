// The shape of a library only editor code uses (AppleSiliconHarmony's
// patcher): no platform restriction, so Unity counts it as a Player
// assembly, but `autoReferenced: false` and referenced by an Editor-only
// assembly alone. Its unsafe code is nothing MenSharp could compile, so the
// collector must not read it — no Player assembly reaches it.
public static class EditorPatcher
{
    public static unsafe int Deref(int value)
    {
        int* pointer = &value;
        return *pointer;
    }
}
