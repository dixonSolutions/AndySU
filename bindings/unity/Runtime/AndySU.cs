// AndySU for Unity (Android). Talks to the host's com.andysu.api.Host over JNI; on other
// platforms and without an AndySU host every call returns neutral values.
using System;
using UnityEngine;

namespace AndySU
{
    public enum ApplyPolicy { Live, Restart, NextStart }

    public enum UpdateMode { Auto, DownloadOnly, CheckOnly, Off }

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

        // The update client when the host is Host API level 3 or newer, which added setMode/mode.
        // On an older host those methods do not exist, so null means "do nothing".
        static AndroidJavaObject ModeClient()
        {
            var h = HostApi();
            if (h == null || h.Call<int>("apiLevel") < 3) return null;
            return h.Call<AndroidJavaObject>("updates");
        }

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

        /// <summary>
        /// How much automatic updating this install performs; null for the mode configured in the
        /// host. Needs Host API level 3: does nothing on an older host.
        /// </summary>
        public static void SetUpdateMode(UpdateMode? m)
        {
            string v = m switch
            {
                UpdateMode.Auto => "auto",
                UpdateMode.DownloadOnly => "download_only",
                UpdateMode.CheckOnly => "check_only",
                UpdateMode.Off => "off",
                _ => null,
            };
            ModeClient()?.Call("setMode", v);
        }

        /// <summary>The effective update mode; null without a host or on a host older than API level 3.</summary>
        public static UpdateMode? GetUpdateMode()
        {
            switch (ModeClient()?.Call<string>("mode"))
            {
                case "auto": return UpdateMode.Auto;
                case "download_only": return UpdateMode.DownloadOnly;
                case "check_only": return UpdateMode.CheckOnly;
                case "off": return UpdateMode.Off;
                default: return null;
            }
        }
    }
}
