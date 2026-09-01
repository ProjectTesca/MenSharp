// MenSharp: the attributes that describe networking, spelled the way
// UdonSharp spells them so the same source reads the same.
//
// These exist for two audiences. Unity's C# compiler needs the types to exist
// at all, or a `[UdonSynced]` in your source is an error before MenSharp ever
// sees it. The MenSharp compiler reads them off the syntax and turns them into
// the `.sync` directives and behaviour sync mode of the Udon program — nothing
// here runs.

using System;

namespace MenSharp
{
    /// How a synced variable is interpolated between updates. `None` means the
    /// value simply arrives; `Linear` and `Smooth` only apply to the numeric
    /// and vector types Udon can interpolate.
    public enum UdonSyncMode
    {
        None,
        Linear,
        Smooth,
    }

    /// Marks a variable as network-synchronised.
    [AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
    public class UdonSyncedAttribute : Attribute
    {
        public UdonSyncedAttribute(UdonSyncMode mode = UdonSyncMode.None)
        {
            Mode = mode;
        }

        public UdonSyncMode Mode { get; }
    }

    /// Whether the behaviour's synced variables are sent every frame
    /// (`Continuous`) or only when you ask (`Manual`, via
    /// `RequestSerialization()`).
    public enum BehaviourSyncMode
    {
        Continuous,
        Manual,
        None,
    }

    /// Sets the sync mode of the whole behaviour. Without it, a behaviour that
    /// has synced variables is `Continuous`.
    [AttributeUsage(AttributeTargets.Class)]
    public class UdonBehaviourSyncModeAttribute : Attribute
    {
        public UdonBehaviourSyncModeAttribute(BehaviourSyncMode mode)
        {
            Mode = mode;
        }

        public BehaviourSyncMode Mode { get; }
    }

    /// Routes external writes to this field — `SetProgramVariable` from
    /// another program, or network sync — through the named property's
    /// setter instead of landing silently. Writes from your own code go to
    /// the field directly, as in UdonSharp.
    [AttributeUsage(AttributeTargets.Field)]
    public class FieldChangeCallbackAttribute : Attribute
    {
        public FieldChangeCallbackAttribute(string callbackPropertyName)
        {
            CallbackPropertyName = callbackPropertyName;
        }

        public string CallbackPropertyName { get; }
    }
}
