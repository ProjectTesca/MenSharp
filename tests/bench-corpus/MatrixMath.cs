// Bench corpus: 3x3 matrix arithmetic on flat arrays.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    public class MatrixMath : MenSharpBehaviour
    {
        private float[] a = new float[9];
        private float[] b = new float[9];
        private float[] c = new float[9];

        private void Start()
        {
            for (int i = 0; i < 9; i++)
            {
                a[i] = i + 1;
                b[i] = (i * 2) % 5;
            }
        }

        private void Multiply(float[] x, float[] y, float[] result)
        {
            for (int row = 0; row < 3; row++)
            {
                for (int col = 0; col < 3; col++)
                {
                    float sum = 0f;
                    for (int k = 0; k < 3; k++)
                    {
                        sum += x[row * 3 + k] * y[k * 3 + col];
                    }
                    result[row * 3 + col] = sum;
                }
            }
        }

        private float Determinant(float[] m)
        {
            return m[0] * (m[4] * m[8] - m[5] * m[7])
                - m[1] * (m[3] * m[8] - m[5] * m[6])
                + m[2] * (m[3] * m[7] - m[4] * m[6]);
        }

        private void Transpose(float[] m)
        {
            for (int row = 0; row < 3; row++)
            {
                for (int col = row + 1; col < 3; col++)
                {
                    float t = m[row * 3 + col];
                    m[row * 3 + col] = m[col * 3 + row];
                    m[col * 3 + row] = t;
                }
            }
        }

        public void Interact()
        {
            Multiply(a, b, c);
            float det = Determinant(c);
            Transpose(c);
            float trace = c[0] + c[4] + c[8];
            Debug.Log("det " + det + " trace " + trace);
        }
    }
}
