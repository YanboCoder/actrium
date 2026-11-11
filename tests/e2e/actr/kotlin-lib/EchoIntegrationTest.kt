package __PACKAGE__

import android.content.Context
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.actrium.actr.PayloadType
import io.actrium.actr.dsl.ActrRef
import io.actrium.actr.dsl.Manifest
import io.actrium.actr.dsl.awaitShutdown
import io.actrium.actr.dsl.linkedWithMonitoring
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Echo integration test (linked mode).
 *
 * Mirrors the 0.4.x application pattern (see actr-kt-migration.md §5): a linked
 * ActrNode is created from the bundled actr.toml + manifest.toml, started, given
 * time to discover the remote EchoService, then an `echo.EchoService.Echo` RPC is
 * sent and the reply is asserted to equal "Echo: <message>".
 *
 * Protobuf is hand-encoded/decoded (tag 1 = `string`) so the test does not depend
 * on generated Echo* classes — only on the generated UnifiedWorkload /
 * UnifiedLifecycleAdapter (from `actr gen -l kotlin`).
 *
 * Run via `./gradlew connectedDebugAndroidTest` against a booted emulator, with
 * an loran + EchoService host reachable at 10.0.2.2 from the emulator.
 */
@RunWith(AndroidJUnit4::class)
class EchoIntegrationTest {
    companion object {
        private const val TAG = "EchoIntegrationTest"
        private const val ECHO_MESSAGE = "hello-kotlin-e2e"
        private const val ROUTE = "echo.EchoService.Echo"
        private const val DEFAULT_SHORT_BACKGROUND_SECONDS = 5L
        private const val DEFAULT_LONG_BACKGROUND_SECONDS = 60L
    }

    private fun ctx(): Context = InstrumentationRegistry.getInstrumentation().targetContext

    private fun copyAsset(name: String): String {
        val src = InstrumentationRegistry.getInstrumentation().context
        val out = File(ctx().filesDir, name)
        out.parentFile?.mkdirs()
        src.assets.open(name).use { input ->
            out.outputStream().use { output -> input.copyTo(output) }
        }
        return out.absolutePath
    }

    // proto3: field 1, wire type 2 (length-delimited string)
    private fun encodeEchoRequest(message: String): ByteArray {
        val bytes = message.toByteArray(Charsets.UTF_8)
        val out = ByteArray(2 + bytes.size)
        out[0] = 0x0a // tag: (1 << 3) | 2
        out[1] = bytes.size.toByte()
        System.arraycopy(bytes, 0, out, 2, bytes.size)
        return out
    }

    private fun decodeEchoResponse(payload: ByteArray): String {
        if (payload.size < 2) return ""
        val tag = payload[0].toInt() and 0xFF
        if ((tag shr 3) != 1) return "" // expect field 1
        val len = payload[1].toInt() and 0xFF
        return if (payload.size >= 2 + len) String(payload, 2, len, Charsets.UTF_8) else ""
    }

    private fun backgroundDurationSeconds(
        argumentName: String,
        defaultValue: Long,
    ): Long =
        InstrumentationRegistry
            .getArguments()
            .getString(argumentName)
            ?.toLongOrNull()
            ?.takeIf { it >= 0L }
            ?: defaultValue

    private suspend fun assertEcho(
        clientRef: ActrRef,
        message: String,
    ) {
        Log.i(TAG, "Calling $ROUTE with '$message' ...")
        val replyPayload =
            clientRef.call(
                ROUTE,
                PayloadType.RPC_RELIABLE,
                encodeEchoRequest(message),
                30000L,
            )
        val reply = decodeEchoResponse(replyPayload)
        Log.i(TAG, "Reply: $reply")
        assertEquals("Echo: $message", reply)
    }

    @Test
    fun testEchoRecoversAfterShortAndLongBackground(): Unit = runBlocking {
        Log.i(TAG, "=== Kotlin Echo integration test (linked mode + monitoring) ===")
        val config = io.actrium.actr.readRealmRuntimeConfiguration(copyAsset("actr.toml"))
        val enrollmentFile = File(ctx().noBackupFilesDir, "actr-enrollment.json")
        val enrollment = org.json.JSONObject(enrollmentFile.readText())
        val membership = io.actrium.actr.dsl.RealmIdentityStore(ctx()).membership(
            config.realm, enrollment.getString("provider_id"), config.issuer, true)
        membership.join(enrollment.getString("invite"), "Kotlin E2E device")
        check(enrollmentFile.delete())
        membership.members()
        val manifestPath = copyAsset("manifest.toml")
        val shortBackgroundSeconds =
            backgroundDurationSeconds(
                "shortBackgroundSeconds",
                DEFAULT_SHORT_BACKGROUND_SECONDS,
            )
        val longBackgroundSeconds =
            backgroundDurationSeconds(
                "longBackgroundSeconds",
                DEFAULT_LONG_BACKGROUND_SECONDS,
            )
        val monitorScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

        val actorType = Manifest.from(File(manifestPath)).packageType()
        Log.i(TAG, "Actor type: ${actorType.publisher}/${actorType.name}@${actorType.version}")

        val workload = __PACKAGE__.UnifiedWorkload()
        val lifecycle = __PACKAGE__.UnifiedLifecycleAdapter(workload)

        val node = linkedWithMonitoring(
            configPath = manifestPath,
            actorType = actorType,
            workload = lifecycle.toDynamicWorkload(),
            context = ctx(),
            scope = monitorScope,
            membership = membership,
            onNetworkStatusLog = { status -> Log.i(TAG, "Network status: $status") },
        )

        var clientRef: ActrRef? = null
        try {
            clientRef = node.start()
            Log.i(TAG, "Client started: ${clientRef.actorId().serialNumber}")

            // Allow onStart to discover the remote EchoService.
            delay(3000)

            assertEcho(clientRef, ECHO_MESSAGE)
            Log.i(TAG, "✅ Initial echo round-trip succeeded")

            val backgroundDurations =
                listOf(
                    "short" to shortBackgroundSeconds,
                    "long" to longBackgroundSeconds,
                )
            for ((label, durationSeconds) in backgroundDurations) {
                Log.i(TAG, "ACTR_E2E_PHASE:background-$label")
                node.onAppBackground()
                delay(durationSeconds * 1000L)

                Log.i(TAG, "ACTR_E2E_PHASE:foreground-$label")
                node.onAppForeground()
                assertEcho(clientRef, "$ECHO_MESSAGE-$label-background")
                Log.i(TAG, "✅ Echo recovered after $label background (${durationSeconds}s)")
            }
        } finally {
            try {
                clientRef?.shutdown()
                clientRef?.awaitShutdown()
            } catch (e: Exception) {
                Log.w(TAG, "shutdown: ${e.message}")
            } finally {
                clientRef?.close()
                node.close()
                monitorScope.cancel()
            }
        }
    }
}
