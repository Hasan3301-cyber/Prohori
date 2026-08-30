package org.prohori.app

import org.prohori.core.Prohori

/**
 * A deliberately coarse hospital-service request derived from a verified protocol id.
 *
 * This is routing metadata, not a diagnosis. Keeping the mapping deterministic also means
 * no patient wording is copied into Telegram/SMS requests and hospitals never receive a
 * model-invented condition.
 */
data class EmergencyCareTarget(
    val specialty: String,
    val userLabel: String,
)

internal data class EmergencyChatDecision(
    val target: EmergencyCareTarget,
    val response: String,
)

/** Returns null only when deterministic red-flag rules found no emergency signal. */
internal fun emergencyChatDecision(core: Prohori, recentUserContext: String): EmergencyChatDecision? {
    val triage = core.triage(recentUserContext)
    if (triage.hits.isEmpty()) return null
    val target = emergencyCareTarget(triage.card?.protocolId)
    val response =
        buildString {
            append("I recognized a possible emergency signal. I will not diagnose it or let the free-form model improvise.\n\n")
            append("Ask for ${target.userLabel}. This service suggestion is a routing category, not a diagnosis.\n\n")
            triage.card?.let { append(it.plainText) }
                ?: append(
                    "No matching first-aid protocol is available. Call emergency services now and follow the dispatcher’s instructions.",
                )
        }
    return EmergencyChatDecision(target, response)
}

internal fun emergencyCareTarget(protocolId: String?): EmergencyCareTarget =
    when (protocolId) {
        "chest.pain" -> EmergencyCareTarget("cardiac_emergency", "emergency medicine with cardiac support")
        "stroke.suspected", "seizure.active" ->
            EmergencyCareTarget("neurology_emergency", "emergency medicine with neurological support")
        "burn.thermal" -> EmergencyCareTarget("burns", "emergency medicine or a burns service")
        "breathing.distress", "choking.adult", "drowning.rescue", "allergy.anaphylaxis" ->
            EmergencyCareTarget("respiratory_emergency", "emergency medicine with airway/respiratory support")
        "bleeding.severe", "fracture.suspected", "head.injury", "electric.shock", "snake.bite" ->
            EmergencyCareTarget("trauma_emergency", "emergency medicine or a trauma service")
        "cpr.adult", "unresponsive.breathing" ->
            EmergencyCareTarget("resuscitation", "an emergency/resuscitation team")
        "poisoning.swallowed" -> EmergencyCareTarget("toxicology_emergency", "emergency medicine or poison support")
        "heat.illness", "dehydration.diarrhoea" ->
            EmergencyCareTarget("general_emergency", "an emergency medicine clinician")
        else -> EmergencyCareTarget("general_emergency", "an emergency medicine clinician")
    }

internal fun specialtyDisplayName(specialty: String): String =
    specialty.replace('_', ' ').replaceFirstChar { it.uppercase() }
