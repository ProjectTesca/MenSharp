#if UNITY_EDITOR
using NUnit.Framework;

/// MenSharpSceneProcessor.CallbackOrder is a public constant other scene
/// processors order themselves against, so it must stay a compile-time value
/// and keep agreeing with the callbackOrder the processor actually reports.
public class MenSharpSceneProcessorTests
{
    [Test]
    public void TheCallbackOrderConstantIsStableAndVeryEarly()
    {
        Assert.AreEqual(-10_000, MenSharpSceneProcessor.CallbackOrder);
    }

    [Test]
    public void TheReportedOrderMatchesThePublishedConstant()
    {
        Assert.AreEqual(
            MenSharpSceneProcessor.CallbackOrder,
            new MenSharpSceneProcessor().callbackOrder);
    }
}
#endif
