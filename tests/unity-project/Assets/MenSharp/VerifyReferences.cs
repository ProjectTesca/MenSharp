// Types from assemblies the compiler has to find on its own: a package's
// script assembly (Cinemachine, AI Navigation, post-processing), the SDK's
// Dynamics dlls, the economy dll, and mono's System.dll. Compile-only: what
// matters is that every name resolves and every extern is spelled as the
// whitelist has it (`ReleaseGrabs` on VRCPhysBone, not on its base).
using MenSharp;
public class VerifyReferences : MenSharpBehaviour
{
    public void TestEconomy(string id)
        => VRC.Economy.Store.OpenGroupPage(id);
    public void TestSystem(System.Diagnostics.Stopwatch stopwatch)
        => stopwatch.Start();
    public void TestCinemachine(Cinemachine.CinemachinePathBase path)
        => path.InvalidateDistanceCache();
    public void TestNavigation(Unity.AI.Navigation.NavMeshLink link)
        => link.UpdateLink();
    public UnityEngine.Component TestPostProcessing(
        UnityEngine.Rendering.PostProcessing.PostProcessVolume volume)
        => volume.GetComponent(typeof(UnityEngine.Transform));
    public void TestDynamics(VRC.Dynamics.VRCConstraintSourceKeyableList sources)
        => sources.Clear();
    public void TestConstraint(
        VRC.SDK3.Dynamics.Constraint.Components.VRCAimConstraint constraint)
        => constraint.ZeroConstraint();
    public void TestContact(
        VRC.SDK3.Dynamics.Contact.Components.VRCContactReceiver receiver)
        => receiver.ApplyConfigurationChanges();
    public void TestPhysBone(
        VRC.SDK3.Dynamics.PhysBone.Components.VRCPhysBone physBone)
        => physBone.ReleaseGrabs();
}
