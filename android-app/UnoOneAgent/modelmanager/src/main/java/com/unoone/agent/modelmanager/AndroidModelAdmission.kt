package com.unoone.agent.modelmanager

import android.app.ActivityManager
import android.content.Context
import android.net.ConnectivityManager
import android.os.Build
import android.os.StatFs
import com.unoone.agent.core.model.BrainModelRegistry
import com.unoone.agent.core.model.BrainRuntime
import com.unoone.agent.core.modeladmission.ManualAdmission
import com.unoone.agent.core.modeladmission.ManualAdmission.Observation
import com.unoone.agent.core.modeladmission.FileReceiptStore
import com.unoone.agent.core.modeladmission.LoadAdmission
import com.unoone.agent.core.modeladmission.LoadEvidence
import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import com.unoone.agent.core.modeladmission.ReceiptStore
import com.unoone.agent.core.runtime.AgentRuntimeGate
import java.io.File
import java.util.zip.ZipFile

/** Native-only probe and device-local manual consent. WorkManager input is NEVER an approval. */
class AndroidModelAdmission(private val context: Context, private val modelRoot: File) {
    private val prefs get() = context.getSharedPreferences("model-manual-risk-v1", Context.MODE_PRIVATE)
    /** Descriptor identity (bundle store binding) and the Rust-core-aligned working-set profile. */
    fun identity(d: ModelDescriptor) = ModelMemoryProfile.identity(d)
    fun profile(d: ModelDescriptor): ManualAdmission.Profile = ModelMemoryProfile.profile(d)

    /** Device-local receipts: the only producer of "Works here" evidence. */
    val receipts: ReceiptStore by lazy { FileReceiptStore(modelRoot) }

    /** Local fingerprint hash (build fingerprint + total RAM + primary ABI). Never uploaded or displayed. */
    fun deviceFingerprint(): String {
        val total = runCatching {
            val manager = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
            ActivityManager.MemoryInfo().also(manager::getMemoryInfo).totalMem
        }.getOrDefault(-1L)
        val abi = Build.SUPPORTED_ABIS.firstOrNull().orEmpty()
        return ModelBundleStore.sha256("${Build.FINGERPRINT}\n$total\n$abi".toByteArray())
    }

    fun evidence(d: ModelDescriptor, legacyInstalled: Boolean): LoadEvidence =
        receipts.evidence(d.id, identity(d), deviceFingerprint(), legacyInstalled)

    fun record(d: ModelDescriptor, outcome: NativeLoadReceipt.Outcome, lane: String, reason: String = "",
        backend: String = "", loadMs: Long = 0, smokeMs: Long = 0, smokeTokens: Int = 0,
        probe: ManualAdmission.Probe? = null): NativeLoadReceipt {
        val receipt = NativeLoadReceipt(modelId = d.id, identity = identity(d), deviceFingerprint = deviceFingerprint(),
            outcome = outcome, reason = reason, backend = backend, loadMs = loadMs, smokeMs = smokeMs, smokeTokens = smokeTokens,
            totalRamBytes = probe?.totalRam?.measured(), availableRamBytesBefore = probe?.availableRam?.measured(),
            recordedAtMs = System.currentTimeMillis(), lane = lane)
        receipts.record(receipt)
        return receipt
    }
    private fun <T> measure(block: () -> T): Observation<T> = try { Observation.detected(block()) }
        catch (_: Exception) { Observation.unknown() }
    fun probe(): ManualAdmission.Probe {
        val captured = System.currentTimeMillis()
        val memory = runCatching {
            val manager = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
            ActivityManager.MemoryInfo().also(manager::getMemoryInfo)
        }.getOrNull()
        fun memoryValue(read: (ActivityManager.MemoryInfo) -> Long): Observation<Long> =
            memory?.let { Observation.detected(read(it)) } ?: Observation.unknown()
        return ManualAdmission.Probe(captured,
            memoryValue { it.totalMem }, memoryValue { it.availMem }, memoryValue { it.threshold },
            memory?.let { Observation.detected(it.lowMemory) } ?: Observation.unknown(),
            measure { val r = Runtime.getRuntime(); r.maxMemory() - (r.totalMemory() - r.freeMemory()) },
            measure { check(modelRoot.isDirectory || modelRoot.mkdirs()); StatFs(modelRoot.absolutePath).availableBytes },
            Observation.detected(Build.VERSION.SDK_INT),
            measure { check(Build.SUPPORTED_ABIS.isNotEmpty()); Build.SUPPORTED_ABIS.first() },
            measure {
                val rows = File("/proc/cpuinfo").readLines().filter { it.substringBefore(':').trim().equals("Features", true) }
                check(rows.isNotEmpty())
                rows.map { it.substringAfter(':').trim().split(Regex("\\s+")).toSet() }.reduce { a, b -> a intersect b }
            },
            measure {
                val names = File(context.applicationInfo.nativeLibraryDir).listFiles()?.filter { it.isFile }?.map { it.name }?.toMutableSet() ?: mutableSetOf()
                // extractNativeLibs=false: inspect actual APK/split ZIP entries instead of claiming missing libraries.
                val abi = Build.SUPPORTED_ABIS.first()
                (listOf(context.applicationInfo.sourceDir) + context.applicationInfo.splitSourceDirs.orEmpty()).forEach { path ->
                    ZipFile(path).use { zip -> zip.entries().asSequence().filter { it.name.startsWith("lib/$abi/") && !it.isDirectory }
                        .forEach { names += it.name.substringAfterLast('/') } }
                }
                names.toSet()
            })
    }
    private fun policy(p: ManualAdmission.Profile): ManualAdmission.RiskPolicy? {
        val key = p.identity
        if (!prefs.getBoolean("$key.accepted", false)) return null
        return ManualAdmission.RiskPolicy(key, prefs.getInt("$key.context", -1),
            prefs.getLong("$key.issued", 0), prefs.getLong("$key.expires", 0),
            prefs.getLong("$key.download", 0), prefs.getLong("$key.store", 0),
            prefs.getBoolean("$key.metered", false), true)
    }
    /** Called ONLY by explicit native UI confirmation, not by worker or synced/renderer input. */
    fun approveManual(d: ModelDescriptor, allowMetered: Boolean): Boolean {
        val p = profile(d); val now = System.currentTimeMillis()
        val current = storeBytes(p) ?: return false
        val installed = p.installedBytes ?: return false
        val cap = try { Math.addExact(current, Math.addExact(Math.addExact(p.downloadBytes, installed), p.diskHeadroom)) }
            catch (_: ArithmeticException) { return false }
        return prefs.edit().putBoolean("${p.identity}.accepted", true)
            .putInt("${p.identity}.context", p.contextTokens).putLong("${p.identity}.issued", now)
            .putLong("${p.identity}.expires", now + 24 * 60 * 60 * 1000L)
            .putLong("${p.identity}.download", p.downloadBytes).putLong("${p.identity}.store", cap)
            .putBoolean("${p.identity}.metered", allowMetered).commit()
    }
    fun revoke(d: ModelDescriptor) { check(prefs.edit().putBoolean("${identity(d)}.accepted", false).commit()) }
    private fun storeBytes(profile: ManualAdmission.Profile): Long? = runCatching {
        // Reservation already covers this pending attempt. Never credit the old complete bundle.
        val pending = File(modelRoot, ".bundles-v1/${profile.modelId}/partial-${profile.identity}")
        modelRoot.walkTopDown().onEnter { it != pending }.filter { it.isFile }
            .fold(0L) { n, f -> Math.addExact(n, f.length()) }
    }.getOrNull()
    private var recentProbe: ManualAdmission.Probe? = null
    data class Check(val profile: ManualAdmission.Profile, val probe: ManualAdmission.Probe,
        val decision: ManualAdmission.Decision, val physicalPreview: ManualAdmission.Decision, val ordinaryStatus: String = "NOT_YET_QUALIFIED",
        val evidence: LoadAdmission.EvidenceState = LoadAdmission.EvidenceState.UNKNOWN) {
        fun evidenceLabel(): String = when (evidence) {
            LoadAdmission.EvidenceState.QUALIFIED -> "Qualified (signed physical-device record)"
            LoadAdmission.EvidenceState.WORKS_HERE -> "Works here (native load + smoke recorded on this device; unsigned)"
            LoadAdmission.EvidenceState.UNKNOWN -> "Unknown (no load evidence on this device; this is not a refusal)"
        }
        fun summary(): String = "Evidence: ${evidenceLabel()}.\nOrdinary recommendation: $ordinaryStatus — no signed qualification records.\n" +
            "${profile.downloadBytes} bytes download; installed bytes ${profile.installedBytes ?: "UNKNOWN"}; context ${profile.contextTokens}.\n" +
            "Available RAM ${probe.availableRam.value ?: "UNKNOWN"}; low-memory threshold ${probe.lowMemoryThreshold.value ?: "UNKNOWN"}; " +
            "manual working-set ESTIMATE ${profile.workingSetEstimate ?: "UNKNOWN"}; required headroom ${physicalPreview.requiredAvailable ?: "UNKNOWN"}.\n" +
            "Native process ceiling UNKNOWN; library presence is detected only, not load validation.\nHardware preflight: ${physicalPreview.reason}. Manual lane: ${decision.reason}. " +
            "Download only stages; activation waits for retained-old native load + smoke routing. No device performance guarantee."
    }
    fun check(d: ModelDescriptor, loading: Boolean = false): Check {
        val p = profile(d)
        // Never change capturedAt to make old observations look fresh. Short reuse avoids APK scans
        // per network buffer; current local consent is still read on every call.
        val now = System.currentTimeMillis()
        val probe = recentProbe?.takeIf { !loading && now >= it.capturedAtMs && now - it.capturedAtMs < 1000 }
            ?: probe().also { recentProbe = it }
        val network = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        val connected = runCatching { network?.activeNetwork != null }.getOrDefault(false)
        val metered = runCatching { network?.isActiveNetworkMetered }.getOrNull()
        val current = storeBytes(p)
        val previewOnly = ManualAdmission.RiskPolicy(p.identity, p.contextTokens, now, now + 1000,
            p.downloadBytes, Long.MAX_VALUE, true, true)
        val preview = ManualAdmission.assess(p, probe, previewOnly, now, AgentRuntimeGate.isEnabled(),
            connected, metered, current, loading)
        return Check(p, probe, ManualAdmission.assess(p, probe, policy(p), now,
            AgentRuntimeGate.isEnabled(), connected, metered, current, loading), preview,
            evidence = LoadAdmission.evidenceState(evidence(d, legacyInstalled = false)))
    }

    /** One decision function for load/boot/activation; see [LoadAdmission]. */
    fun decide(d: ModelDescriptor, purpose: LoadAdmission.Purpose, legacyInstalled: Boolean = false): LoadAdmission.Decision {
        val p = profile(d)
        val now = System.currentTimeMillis()
        val probe = probe().also { recentProbe = it }
        val network = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        val connected = runCatching { network?.activeNetwork != null }.getOrDefault(false)
        val metered = runCatching { network?.isActiveNetworkMetered }.getOrNull()
        return LoadAdmission.decide(p, probe, policy(p), evidence(d, legacyInstalled), purpose, now,
            AgentRuntimeGate.isEnabled(), connected, metered, storeBytes(p))
    }
}
