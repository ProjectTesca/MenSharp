// Finding behaviours on GameObjects: `GetComponent<Door>()` and its family
// for a type argument that is a *program* — a behaviour of the user's, or an
// UdonSharp behaviour.
//
// Udon's `GetComponent(typeof(...))` can only name engine types, so a program
// is found the way UdonSharp finds one: every UdonBehaviour on the object is
// asked, by name, for its identity variable — `__program_id` on a MenSharp
// program, `__refl_typeid`/`__refl_typeids` on an UdonSharp one — and the
// first (or every) one that answers with the id of the wanted type is it.
//
// The searches below are ordinary M# code, monomorphized per type argument.
// What they lean on are five intrinsics the compiler lowers in place:
// `BehavioursOf`/`BehavioursInChildren`/`BehavioursInParent` (the engine
// call with `typeof(UdonBehaviour)`), `Is<T>` (does this behaviour carry
// T's id — which ids to look for is a compile-time question) and `As<T>` (a
// behaviour reference typed as T). Their bodies here are placeholders.
namespace MenSharp.Internal
{
    public static class Programs
    {
        // ------------------------------------------------- intrinsics

        /// `transform.GetComponents(typeof(UdonBehaviour))`.
        public static object[] BehavioursOf(object transform)
        {
            return null;
        }

        public static object[] BehavioursInChildren(object transform, bool includeInactive)
        {
            return null;
        }

        public static object[] BehavioursInParent(object transform, bool includeInactive)
        {
            return null;
        }

        /// Is this UdonBehaviour a program of type `T` (or of a subclass)?
        public static bool Is<T>(object behaviour)
        {
            return false;
        }

        /// The behaviour as a `T` reference.
        public static T As<T>(object behaviour)
        {
            return default(T);
        }

        // ------------------------------------- the scheduler's intrinsics

        /// `SendCustomEventDelayedSeconds("_mensharpResume", seconds)` on
        /// this behaviour: how `Task.Delay` gets called back.
        public static void ScheduleResume(float seconds)
        {
        }

        /// `SendCustomEventDelayedFrames("_mensharpResume", frames)`.
        public static void ScheduleResumeFrames(int frames)
        {
        }

        /// `Time.time`.
        public static float Now()
        {
            return 0f;
        }

        /// `Time.frameCount`.
        public static int FrameCount()
        {
            return 0;
        }

        /// `Debug.LogError`.
        public static void LogError(object message)
        {
        }

        // ------------------------------------------------- the searches

        public static T GetComponent<T>(object transform)
        {
            return First<T>(BehavioursOf(transform));
        }

        public static T[] GetComponents<T>(object transform)
        {
            return All<T>(BehavioursOf(transform));
        }

        public static T GetComponentInChildren<T>(object transform, bool includeInactive)
        {
            return First<T>(BehavioursInChildren(transform, includeInactive));
        }

        public static T[] GetComponentsInChildren<T>(object transform, bool includeInactive)
        {
            return All<T>(BehavioursInChildren(transform, includeInactive));
        }

        public static T GetComponentInParent<T>(object transform, bool includeInactive)
        {
            return First<T>(BehavioursInParent(transform, includeInactive));
        }

        public static T[] GetComponentsInParent<T>(object transform, bool includeInactive)
        {
            return All<T>(BehavioursInParent(transform, includeInactive));
        }

        private static T First<T>(object[] behaviours)
        {
            if (behaviours == null)
            {
                return default(T);
            }
            for (int i = 0; i < behaviours.Length; i++)
            {
                if (Is<T>(behaviours[i]))
                {
                    return As<T>(behaviours[i]);
                }
            }
            return default(T);
        }

        private static T[] All<T>(object[] behaviours)
        {
            int count = 0;
            if (behaviours != null)
            {
                for (int i = 0; i < behaviours.Length; i++)
                {
                    if (Is<T>(behaviours[i]))
                    {
                        count++;
                    }
                }
            }
            T[] found = new T[count];
            int next = 0;
            if (behaviours != null)
            {
                for (int i = 0; i < behaviours.Length; i++)
                {
                    if (Is<T>(behaviours[i]))
                    {
                        found[next] = As<T>(behaviours[i]);
                        next++;
                    }
                }
            }
            return found;
        }
    }
}
