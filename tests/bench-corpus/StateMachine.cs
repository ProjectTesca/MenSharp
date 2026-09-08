// Bench corpus: a door state machine driven by interact and time.
using MenSharp;
using UnityEngine;

namespace MenSharpBench
{
    // top level: UdonSharp does not accept nested type declarations
    public enum DoorState { Closed, Opening, Open, Closing, Locked }

    public class StateMachine : MenSharpBehaviour
    {
        public float travelSeconds = 1.5f;
        public float stayOpenSeconds = 4f;
        public bool startLocked;
        private DoorState state;
        private float clock;
        private int transitions;

        private void Start()
        {
            state = startLocked ? DoorState.Locked : DoorState.Closed;
        }

        public void Interact()
        {
            switch (state)
            {
                case DoorState.Closed:
                    Go(DoorState.Opening);
                    break;
                case DoorState.Open:
                    Go(DoorState.Closing);
                    break;
                case DoorState.Locked:
                    Debug.Log("locked");
                    break;
                default:
                    break;
            }
        }

        public void Unlock()
        {
            if (state == DoorState.Locked)
            {
                Go(DoorState.Closed);
            }
        }

        private void Update()
        {
            clock += Time.deltaTime;
            if (state == DoorState.Opening && clock >= travelSeconds)
            {
                Go(DoorState.Open);
            }
            else if (state == DoorState.Open && clock >= stayOpenSeconds)
            {
                Go(DoorState.Closing);
            }
            else if (state == DoorState.Closing && clock >= travelSeconds)
            {
                Go(DoorState.Closed);
            }
            float openness = Openness();
            transform.localScale = new Vector3(1f, 1f - openness * 0.9f, 1f);
        }

        private void Go(DoorState next)
        {
            state = next;
            clock = 0f;
            transitions++;
        }

        private float Openness()
        {
            float t = travelSeconds > 0f ? Mathf.Clamp01(clock / travelSeconds) : 1f;
            if (state == DoorState.Opening)
            {
                return t;
            }
            if (state == DoorState.Closing)
            {
                return 1f - t;
            }
            return state == DoorState.Open ? 1f : 0f;
        }

        public int Transitions()
        {
            return transitions;
        }
    }
}
