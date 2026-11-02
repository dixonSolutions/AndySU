package com.andysu.kit

import android.app.Activity
import android.os.Bundle
import android.view.View
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.savedstate.SavedStateRegistry
import androidx.savedstate.SavedStateRegistryController
import androidx.savedstate.SavedStateRegistryOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import com.andysu.api.ActivityEvent

/**
 * Lifecycle + saved-state owners for one host stub Activity, owned by the payload generation.
 * Install them on the root view before `setContentView`, and forward the host's activity events
 * with [onEvent]. A new generation after a live swap creates fresh owners (its own AndroidX
 * classes), so nothing from the previous generation leaks into it.
 */
class ActivityOwners(activity: Activity, saved: Bundle?) : LifecycleOwner, SavedStateRegistryOwner {
    private val registry = LifecycleRegistry(this)
    private val controller = SavedStateRegistryController.create(this)

    override val lifecycle: Lifecycle get() = registry
    override val savedStateRegistry: SavedStateRegistry get() = controller.savedStateRegistry

    init {
        controller.performAttach()
        controller.performRestore(saved)
        registry.currentState = Lifecycle.State.CREATED
        // Attached after the Activity already started (e.g. a live swap): catch up.
        if (activity.hasWindowFocus() || !activity.isFinishing) registry.currentState = Lifecycle.State.RESUMED
    }

    fun install(root: View): View {
        root.setViewTreeLifecycleOwner(this)
        root.setViewTreeSavedStateRegistryOwner(this)
        return root
    }

    fun onEvent(event: Int) {
        registry.currentState = when (event) {
            ActivityEvent.START, ActivityEvent.PAUSE -> Lifecycle.State.STARTED
            ActivityEvent.RESUME -> Lifecycle.State.RESUMED
            ActivityEvent.STOP -> Lifecycle.State.CREATED
            ActivityEvent.DESTROY -> Lifecycle.State.DESTROYED
            else -> return
        }
    }

    /** The generation is being suspended (live swap): tear down its composition. */
    fun destroy() {
        if (registry.currentState != Lifecycle.State.DESTROYED) registry.currentState = Lifecycle.State.DESTROYED
    }
}
