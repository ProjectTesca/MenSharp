using MenSharp;
using VRC.Udon;

public class MenSharpRuntimeCaller : MenSharpBehaviour
{
    public MenSharpRuntimeTarget target;
    public bool done;
    public int result;

    // SetProgramVariable<T> is C# sugar over the non-generic (string, object)
    // extern; passing an int[] used to fail because M# tried to name int[] as a
    // System.Type. Set on another behaviour, read back by the test.
    public UdonBehaviour receiver;

    public void RunRemote()
    {
        result = target.Add(20, 22);
        done = true;
    }

    // Component/Behaviour/Object members on the behaviour itself and on
    // the behaviour it holds: at run time both are UdonBehaviours
    public string selfName;
    public string targetName;
    public string targetObjectName;
    public string targetTransformName;
    public string targetText;
    public bool targetWasEnabled;
    public bool targetDisabled;
    public bool targetReenabled;
    public int selfId;
    public int targetId;
    public bool idsDiffer;
    public bool sameAsSelf;
    public bool sameAsTarget;
    public bool nullIsNull;
    public bool alive;
    public bool notAlive;
    public bool deadIsNull;
    public bool foundViaComponent;
    public bool foundSelfViaComponent;
    public bool andAlive;

    public void EngineMembers()
    {
        selfName = name;
        targetName = target.name;
        targetObjectName = target.gameObject.name;
        targetTransformName = target.transform.name;
        targetText = target.ToString();
        targetWasEnabled = target.isActiveAndEnabled;
        target.enabled = false;
        targetDisabled = !target.isActiveAndEnabled;
        target.enabled = true;
        targetReenabled = target.isActiveAndEnabled && target.enabled;
        selfId = GetInstanceID();
        targetId = target.GetInstanceID();
        idsDiffer = selfId != targetId && target.GetHashCode() == targetId;
        sameAsSelf = target == this;
        sameAsTarget = target == target && !(target != target) && target.Equals(target);
        MenSharpRuntimeTarget none = null;
        nullIsNull = none == null && !(none != null);
        alive = target ? true : false;
        notAlive = !target;
        deadIsNull = !none && none == null;
        foundViaComponent = target.GetComponent<MenSharpRuntimeTarget>() == target;
        foundSelfViaComponent = GetComponent<MenSharpRuntimeCaller>() == this;
        andAlive = target && enabled;
    }

    public void PushArray()
    {
        int[] values = { 7, 8, 9 };
        receiver.SetProgramVariable("received", values);
    }
}
