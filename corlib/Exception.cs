// MenSharp mini-corlib: System.Exception and the standard exceptions.
//
// Udon whitelists no exception constructor, so these are ordinary M#
// classes compiled with user code (like List<T>), thrown and caught by the
// compiler's own unwinding. Derive from them as from the .NET ones:
//
//     class DoorLockedException : InvalidOperationException { ... }
//
// `ToString()` is left to the object model: an exception class that does not
// override it prints as its type name, and the unhandled-exception report
// adds `: Message` after it.

namespace System
{
    public class Exception
    {
        private readonly string message;
        private readonly Exception inner;
        // written by the compiler: the type name at `new`, the throw site
        // at `throw`, and one `\n   at ...` line per call the exception
        // unwound through
        private string __type;
        private string __site;
        private string __trace;

        public Exception() { message = null; inner = null; }
        public Exception(string message) { this.message = message; inner = null; }
        public Exception(string message, Exception innerException) { this.message = message; inner = innerException; }

        public virtual string Message => message == null ? "" : message;
        public Exception InnerException => inner;

        /// Where it was thrown and what it unwound through, one frame per
        /// line as .NET prints them; empty until thrown.
        public string StackTrace
        {
            get
            {
                string trace = __site == null ? "" : "   at " + __site;
                if (__trace != null) { trace = trace + __trace; }
                return trace;
            }
        }

        public override string ToString()
        {
            string text = __type == null ? "System.Exception" : __type;
            string message = Message;
            if (message != "") { text = text + ": " + message; }
            string trace = StackTrace;
            if (trace != "") { text = text + "\n" + trace; }
            return text;
        }
    }

    public class SystemException : Exception
    {
        public SystemException() { }
        public SystemException(string message) : base(message) { }
        public SystemException(string message, Exception innerException) : base(message, innerException) { }
    }

    public class InvalidOperationException : SystemException
    {
        public InvalidOperationException() : base("Operation is not valid due to the current state of the object.") { }
        public InvalidOperationException(string message) : base(message) { }
        public InvalidOperationException(string message, Exception innerException) : base(message, innerException) { }
    }

    public class ArgumentException : SystemException
    {
        private readonly string paramName;

        public ArgumentException() : base("Value does not fall within the expected range.") { }
        public ArgumentException(string message) : base(message) { }
        public ArgumentException(string message, Exception innerException) : base(message, innerException) { }
        public ArgumentException(string message, string paramName) : base(message) { this.paramName = paramName; }

        public string ParamName => paramName;
    }

    public class ArgumentNullException : ArgumentException
    {
        public ArgumentNullException() : base("Value cannot be null.") { }
        public ArgumentNullException(string paramName) : base("Value cannot be null.", paramName) { }
        public ArgumentNullException(string paramName, string message) : base(message, paramName) { }
    }

    public class ArgumentOutOfRangeException : ArgumentException
    {
        public ArgumentOutOfRangeException() : base("Specified argument was out of the range of valid values.") { }
        public ArgumentOutOfRangeException(string paramName) : base("Specified argument was out of the range of valid values.", paramName) { }
        public ArgumentOutOfRangeException(string paramName, string message) : base(message, paramName) { }
    }

    public class IndexOutOfRangeException : SystemException
    {
        public IndexOutOfRangeException() : base("Index was outside the bounds of the array.") { }
        public IndexOutOfRangeException(string message) : base(message) { }
    }

    public class NullReferenceException : SystemException
    {
        public NullReferenceException() : base("Object reference not set to an instance of an object.") { }
        public NullReferenceException(string message) : base(message) { }
    }

    public class InvalidCastException : SystemException
    {
        public InvalidCastException() : base("Specified cast is not valid.") { }
        public InvalidCastException(string message) : base(message) { }
    }

    public class DivideByZeroException : SystemException
    {
        public DivideByZeroException() : base("Attempted to divide by zero.") { }
        public DivideByZeroException(string message) : base(message) { }
    }

    public class NotSupportedException : SystemException
    {
        public NotSupportedException() : base("Specified method is not supported.") { }
        public NotSupportedException(string message) : base(message) { }
    }

    public class OperationCanceledException : SystemException
    {
        public OperationCanceledException() : base("The operation was canceled.") { }
        public OperationCanceledException(string message) : base(message) { }
    }

    public class NotImplementedException : SystemException
    {
        public NotImplementedException() : base("The method or operation is not implemented.") { }
        public NotImplementedException(string message) : base(message) { }
    }

    public class FormatException : SystemException
    {
        public FormatException() : base("One of the identified items was in an invalid format.") { }
        public FormatException(string message) : base(message) { }
    }

    public class OverflowException : SystemException
    {
        public OverflowException() : base("Arithmetic operation resulted in an overflow.") { }
        public OverflowException(string message) : base(message) { }
    }
}

namespace System.Collections.Generic
{
    public class KeyNotFoundException : SystemException
    {
        public KeyNotFoundException() : base("The given key was not present in the dictionary.") { }
        public KeyNotFoundException(string message) : base(message) { }
    }
}

namespace System.Threading.Tasks
{
    public class TaskCanceledException : OperationCanceledException
    {
        public TaskCanceledException() : base("A task was canceled.") { }
        public TaskCanceledException(string message) : base(message) { }
    }
}

namespace System.Runtime.CompilerServices
{
    public class SwitchExpressionException : InvalidOperationException
    {
        public SwitchExpressionException() : base("Non-exhaustive switch expression failed to match its input.") { }
        public SwitchExpressionException(string message) : base(message) { }
    }
}
