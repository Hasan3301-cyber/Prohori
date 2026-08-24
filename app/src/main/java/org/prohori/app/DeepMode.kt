package org.prohori.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import org.prohori.core.ModelAssessment
import org.prohori.core.Prohori
import org.prohori.core.Triage
import java.io.File

data class LocationFix(
    val zone: String,
    val precisionLabel: String,
    val ageSeconds: Long,
)

data class LocalDirection(
    val hospitalId: String,
    val hospitalName: String,
    val routeSummary: String,
    val conditionAgeLabel: String,
)

data class DeepResult(
    val immediate: Triage,
    val model: ModelAssessment?,
    val location: LocationFix?,
    val direction: LocalDirection?,
)

/**
 * P6 fan-out: deterministic rules return before this function is launched; inside Deep
 * mode the single model worker overlaps only with location/pack/graph work. Callers inject
 * platform location and signed-pack routing so this class neither requests permissions nor
 * invents a facility when those services fail.
 */
class DeepCoordinator(
    private val core: Prohori,
    private val modelFile: File?,
) {
    suspend fun run(
        message: String,
        locate: suspend () -> LocationFix?,
        route: suspend (LocationFix) -> LocalDirection?,
    ): DeepResult = coroutineScope {
        val immediate = core.triage(message)
        val modelTask =
            async(Dispatchers.Default.limitedParallelism(1)) {
                modelFile?.takeIf(File::isFile)?.let { OnDeviceEngine.assess(core, it, message) }
            }
        val localTask =
            async(Dispatchers.Default) {
                val fix = locate()
                fix to fix?.let { route(it) }
            }
        val (location, direction) = localTask.await()
        DeepResult(immediate, modelTask.await(), location, direction)
    }
}
