package com.andysu.host

import android.app.Application
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.os.Build
import kotlin.concurrent.thread

/**
 * Periodic background checks through the platform JobScheduler (no WorkManager in the frozen
 * host, see docs/DECISIONS.md). Constraints come from the config: unmetered network, charging,
 * battery not low. The app also checks on every open, since OEM battery savers delay jobs.
 */
object Jobs {
    const val JOB_ID = 0x414e4459

    fun schedule(ctx: Context, cfg: Config, mode: String = cfg.mode) {
        val js = ctx.getSystemService(Context.JOB_SCHEDULER_SERVICE) as JobScheduler
        if (mode == "off") {
            js.cancel(JOB_ID)
            return
        }
        val b = JobInfo.Builder(JOB_ID, ComponentName(ctx, UpdateJobService::class.java))
            .setRequiredNetworkType(if (cfg.network == "unmetered") JobInfo.NETWORK_TYPE_UNMETERED else JobInfo.NETWORK_TYPE_ANY)
            .setPeriodic(maxOf(15L, cfg.checkIntervalMinutes) * 60_000L)
            .setRequiresCharging(cfg.charging)
            .setPersisted(true)
        if (Build.VERSION.SDK_INT >= 26) b.setRequiresBatteryNotLow(cfg.batteryNotLow)
        val info = b.build()
        val existing = if (Build.VERSION.SDK_INT >= 24) js.getPendingJob(JOB_ID) else js.allPendingJobs.firstOrNull { it.id == JOB_ID }
        if (existing == null || existing.intervalMillis != info.intervalMillis || existing.networkType != info.networkType) js.schedule(info)
    }
}

class UpdateJobService : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        AndySU.init(application as Application)
        thread(name = "andysu-job") {
            try {
                AndySU.updater.runCheck("job")
            } finally {
                jobFinished(params, false)
            }
        }
        return true
    }

    override fun onStopJob(params: JobParameters) = true
}
