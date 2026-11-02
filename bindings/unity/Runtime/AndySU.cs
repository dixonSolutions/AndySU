// AndySU for Unity (Android). Talks to the host's com.andysu.api.Host over JNI; on other
// platforms and without an AndySU host every call returns neutral values.
using System;
using UnityEngine;

namespace AndySU
{
    public enum ApplyPolicy { Live, Restart, NextStart }

    public static class Updates
    {
#if UNITY_ANDROID && !UNITY_EDITOR
        static AndroidJavaObject HostApi()
        {
            using (var host = new AndroidJavaClass("com.andysu.api.Host"))
                return host.CallStatic<AndroidJavaObject>("current");
        }

        static AndroidJavaObject Client()
        {
            var h = HostApi();
            return h?.Call<AndroidJavaObject>("updates");
        }
#else
        static AndroidJavaObject HostApi() => null;
        static AndroidJavaObject Client() => null;
#endif

        /// <summary>True when the game runs under an AndySU host.</summary>
        public static bool IsPresent => HostApi() != null;

        public static string Version => HostApi()?.Call<string>("version") ?? "";

        /// <summary>Status as JSON, e.g. {"state":"staged","release":"1.4.0"}.</summary>
        public static string StatusJson => Client()?.Call<string>("status") ?? "{\"state\":\"idle\"}";

        public static void CheckNow() => Client()?.Call("checkNow");
        public static void ApplyNow() => Client()?.Call("applyNow");
        public static void RestartNow() => Client()?.Call("restartNow");

        /// <summary>Commit this payload once the game is known to work.</summary>
        public static void MarkHealthy() => HostApi()?.Call("markHealthy");

        /// <summary>How staged updates apply on this install; null for the configured policy.</summary>
        public static void SetApplyPolicy(ApplyPolicy? p)
        {
            string v = p switch
            {
                ApplyPolicy.Live => "live",
                ApplyPolicy.Restart => "restart",
                ApplyPolicy.NextStart => "next_start",
                _ => null,
            };
            Client()?.Call("setApplyPolicy", v);
        }
    }
}
