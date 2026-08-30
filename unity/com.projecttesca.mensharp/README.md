# MenSharp (M#)

A UdonSharp-compatible C# compiler for VRChat Udon, written in Rust.
Classes, generics, `List<T>`, string interpolation and virtual dispatch —
compiled to Udon assembly and validated against the SDK's extern whitelist
at compile time.

## Getting started

1. Install this package with ALCOM or the VRChat Creator Companion.
2. In your project, create `Assets/MenSharp/` and put your `.cs` sources there
   (running **MenSharp > Compile All** once creates the folder for you).
3. List your entry classes in `Assets/MenSharp/mensharp.json`:

   ```json
   { "entries": [ "Demo.Greeter" ] }
   ```

   An entry class' public static methods become Udon events (`Start` →
   `_start`, `Interact` → `_interact`, other names become custom events), and
   its public static fields appear as public variables on the UdonBehaviour.
4. **MenSharp > Compile All** (Ctrl+Shift+M). Diagnostics appear in the
   Console; program assets appear under `Assets/MenSharp/Programs/`.
5. Drop a program asset onto an UdonBehaviour's **Program Source**.

Program assets keep their GUID across recompiles, so scene references never
break.

## Notes

- Your sources are plain C#, so Unity compiles them too — that is what gives
  you IDE completion and it is expected; the Udon program is produced by the
  bundled MenSharp compiler, not by Unity.
- Not supported yet (each is a clear compile error, never silent breakage):
  recursion, exceptions, `ref`/`out` arguments, `base.` access, enums,
  user-defined structs, generic externs such as `GetComponent<T>`.

## Links

- Source, issues: https://github.com/ProjectTesca/MenSharp
