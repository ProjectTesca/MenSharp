using MenSharp;
using UnityEngine;

public class Test1 : MenSharpBehaviour
{
    public Door door;

    public void Start()
    {
        Debug.Log(door.name);
    }
}