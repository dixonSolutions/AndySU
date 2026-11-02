// Minimal stand-ins for the UnityEngine types AndySU.cs uses, so the package compiles with plain
// dotnet in CI (Unity itself needs a licence). Not shipped.
namespace UnityEngine
{
    public class AndroidJavaObject : System.IDisposable
    {
        public T Call<T>(string m, params object[] a) => default;
        public void Call(string m, params object[] a) { }
        public void Dispose() { }
    }
    public class AndroidJavaClass : AndroidJavaObject
    {
        public AndroidJavaClass(string n) { }
        public T CallStatic<T>(string m, params object[] a) => default;
    }
}
