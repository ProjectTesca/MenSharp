# MenSharp (M#)

A UdonSharp-compatible C# compiler for VRChat Udon, written in Rust.
Classes, generics, `List<T>`, string interpolation and virtual dispatch —
compiled to Udon assembly and validated against the SDK's extern whitelist
at compile time.

## Getting started

1. Install this package with ALCOM or the VRChat Creator Companion.
2. In your project, create `Assets/MenSharp/` and put your `.cs` sources there
   (running **MenSharp > Compile All** once creates the folder for you).
3. Write behaviours — any class inheriting `MenSharp.MenSharpBehaviour`
   becomes one Udon program, no configuration needed:

   ```csharp
   using MenSharp;

   public class Door : MenSharpBehaviour
   {
       public int openCount;                  // a public variable on the UdonBehaviour

       public void Interact()                 // Udon events: Start → _start,
       {                                      // Interact → _interact, other
           openCount += 1;                    // names become custom events
       }
   }
   ```

4. Save — compilation runs automatically (or **MenSharp > Compile All**,
   Ctrl+Shift+M). Diagnostics appear in the Console; one program asset per
   behaviour appears under `Assets/MenSharp/Programs/`.
5. Add the behaviour to a GameObject like any component (drag the script or
   Add Component). An UdonBehaviour with the compiled program is paired
   automatically; inspector-edited field values are carried into Udon when
   entering play mode or building.

Program assets keep their GUID across recompiles, so scenes never break.
The component you added is a *proxy*: at play/build time its values transfer
to the paired UdonBehaviour and the proxy itself is stripped, so code never
runs twice.

## Notes

- Your sources are plain C#, so Unity compiles them too — that is what gives
  you IDE completion and it is expected; the Udon program is produced by the
  bundled MenSharp compiler, not by Unity.
- Not supported yet (each is a clear compile error, never silent breakage):
  recursion, exceptions, `ref`/`out` arguments, `base.` access, enums,
  user-defined structs, generic externs such as `GetComponent<T>`.

## Links

- Source, issues: https://github.com/ProjectTesca/MenSharp
