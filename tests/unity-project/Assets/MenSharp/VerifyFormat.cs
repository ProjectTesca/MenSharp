// MenSharp verification: interpolation format specifiers and alignment.
//
// Setup: a Cube "VerifyFormat" with this component. Play, click.
//
// Expected:
//   [verify-format] 1 numbers: 1.2|1.235|1|00042|2A|1,234,567|1.23
//   [verify-format] 2 align: [   42][42   ][    1.23][   ][ab   |  cd]
//   [verify-format] 3 engine: (1.0, 2.0, 3.0) and 01:30
//   [verify-format] 4 plain: no format still works: 42 and 1.234567

using MenSharp;
using UnityEngine;

public class VerifyFormat : MenSharpBehaviour
{
    public void Interact()
    {
        float elapsed = 1.234567f;
        int count = 42;
        long big = 1234567L;
        Debug.Log($"[verify-format] 1 numbers: {elapsed:0.0}|{elapsed:F3}|{elapsed:0}|{count:D5}|{count:X}|{big:N0}|{elapsed:#.##}");

        string missing = null;
        Debug.Log($"[verify-format] 2 align: [{count,5}][{count,-5}][{elapsed,8:0.00}][{missing,3}][{"ab",-5}|{"cd",4}]");

        Vector3 position = new Vector3(1f, 2f, 3f);
        int seconds = 90;
        Debug.Log($"[verify-format] 3 engine: {position:F1} and {seconds / 60:00}:{seconds % 60:00}");

        Debug.Log($"[verify-format] 4 plain: no format still works: {count} and {elapsed}");
    }
}
