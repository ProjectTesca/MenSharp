# Udon node definitions

Machine-generated snapshots of the Udon node definitions (extern signatures,
types, graph nodes) exposed by the VRChat Worlds SDK, one JSON file per Unity
editor version. This is the whitelist the M# code generator compiles against.

## Provenance

Each file is produced by running `tools/unity/MenSharpExternDump.cs` inside a
Unity project with the VRChat Worlds SDK installed (menu:
`MenSharp > Dump Udon Node Definitions`). The dump is a plain enumeration of
`UdonEditorManager.Instance.GetNodeDefinitions()` — factual API metadata
(type / method / parameter names and directions). No SDK code, binaries, or
documentation are included, and none may be committed to this repository.

| File | SDK source | Dumped |
|------|------------|--------|
| `2022.3.22f1.json` | VRChat Worlds SDK (VPM, 2026-08) | 2026-08-30 |
