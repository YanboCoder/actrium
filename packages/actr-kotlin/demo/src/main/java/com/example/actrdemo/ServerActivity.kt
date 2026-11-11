package com.example.actrdemo

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.ColorStateList
import android.graphics.Color
import android.os.Bundle
import android.os.Environment
import android.util.Log
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import com.example.generated.EchoServiceHandler
import echo.Echo.EchoRequest
import echo.Echo.EchoResponse
import io.actrium.actr.ActrType
import io.actrium.actr.CleanupReason
import io.actrium.actr.dsl.ActrContext
import io.actrium.actr.dsl.ActrNode
import io.actrium.actr.dsl.ActrRef
import io.actrium.actr.dsl.ErrorEvent
import io.actrium.actr.dsl.RealmIdentityStore
import io.actrium.actr.dsl.RpcEnvelope
import io.actrium.actr.dsl.Workload
import io.actrium.actr.dsl.dynamicWorkload
import io.actrium.actr.dsl.linkedWithMonitoring
import io.actrium.actr.readRealmRuntimeConfiguration
import io.actrium.demo.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

class ServerActivity : AppCompatActivity() {
    companion object {
        private const val TAG = "ServerActivity"

        // Limit log buffer to avoid exceeding Android clipboard ~1MB transaction limit
        private const val MAX_LOG_CHARS = 500_000
        private const val TAB_SERVER = 0
        private const val TAB_LOGS = 1
    }

    // Tab views
    private lateinit var serverTabButton: Button
    private lateinit var logsTabButton: Button
    private lateinit var serverTabContent: LinearLayout
    private lateinit var logsTabContent: LinearLayout

    // Server tab views
    private lateinit var statusText: TextView
    private lateinit var startButton: Button
    private lateinit var stopButton: Button

    // Logs tab views
    private lateinit var logText: TextView
    private lateinit var scrollView: ScrollView
    private lateinit var copyLogButton: Button
    private lateinit var downloadLogButton: Button
    private lateinit var clearLogButton: Button

    private var serverSystem: ActrNode? = null
    private var serverRef: ActrRef? = null

    // Logcat reader - streams native actr library logs to the UI
    private lateinit var logcatReader: LogcatReader

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_server)

        // Tab views
        serverTabButton = findViewById(R.id.serverTabButton)
        logsTabButton = findViewById(R.id.logsTabButton)
        serverTabContent = findViewById(R.id.serverTabContent)
        logsTabContent = findViewById(R.id.logsTabContent)

        // Server tab views
        statusText = findViewById(R.id.statusText)
        startButton = findViewById(R.id.startButton)
        stopButton = findViewById(R.id.stopButton)

        // Logs tab views
        logText = findViewById(R.id.logText)
        scrollView = findViewById(R.id.scrollView)
        copyLogButton = findViewById(R.id.copyLogButton)
        downloadLogButton = findViewById(R.id.downloadLogButton)
        clearLogButton = findViewById(R.id.clearLogButton)

        initLogcatReader() // Start early to capture actr library's early logs

        serverTabButton.setOnClickListener { switchToTab(TAB_SERVER) }
        logsTabButton.setOnClickListener { switchToTab(TAB_LOGS) }

        startButton.setOnClickListener { startServer() }
        stopButton.setOnClickListener { stopServer() }

        copyLogButton.setOnClickListener {
            val text = logText.text.toString()
            if (text.isNotEmpty()) {
                val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                clipboard.setPrimaryClip(ClipData.newPlainText("actr logs", text))
                Toast.makeText(this, "Logs copied to clipboard", Toast.LENGTH_SHORT).show()
            }
        }

        downloadLogButton.setOnClickListener { downloadLogs() }

        clearLogButton.setOnClickListener {
            logText.text = ""
        }

        switchToTab(TAB_SERVER)
        log("Server activity created")
    }

    private fun switchToTab(tab: Int) {
        val accentColor = Color.parseColor("#1976D2")
        val defaultColor = Color.parseColor("#E0E0E0")

        when (tab) {
            TAB_SERVER -> {
                serverTabContent.visibility = LinearLayout.VISIBLE
                logsTabContent.visibility = LinearLayout.GONE
                serverTabButton.backgroundTintList = ColorStateList.valueOf(accentColor)
                serverTabButton.setTextColor(Color.WHITE)
                logsTabButton.backgroundTintList = ColorStateList.valueOf(defaultColor)
                logsTabButton.setTextColor(Color.BLACK)
            }
            TAB_LOGS -> {
                serverTabContent.visibility = LinearLayout.GONE
                logsTabContent.visibility = LinearLayout.VISIBLE
                logsTabButton.backgroundTintList = ColorStateList.valueOf(accentColor)
                logsTabButton.setTextColor(Color.WHITE)
                serverTabButton.backgroundTintList = ColorStateList.valueOf(defaultColor)
                serverTabButton.setTextColor(Color.BLACK)
            }
        }
    }

    private fun startServer() {
        statusText.text = "Status: Starting linked EchoService"
        startButton.isEnabled = false
        log("Starting EchoService server...")

        lifecycleScope.launch {
            try {
                val runtimeConfigPath = copyAssetToInternalStorage("actr.toml")
                val realmConfig = readRealmRuntimeConfiguration(runtimeConfigPath)
                val provider =
                    getSharedPreferences("realm-membership", MODE_PRIVATE)
                        .getString("provider:${realmConfig.realm}", null)
                        ?: error("Join this Realm using the membership screen first")
                val membership =
                    RealmIdentityStore(this@ServerActivity).membership(
                        realmConfig.realm,
                        provider,
                        realmConfig.issuer,
                        true,
                    )
                membership.members()
                val actorType =
                    ActrType(publisher = "actrium", name = "EchoService", version = "1.0.0")
                val workload = dynamicWorkload(EchoServerWorkload())
                val system =
                    linkedWithMonitoring(
                        configPath = runtimeConfigPath,
                        actorType = actorType,
                        workload = workload,
                        membership = membership,
                        context = this@ServerActivity,
                        scope = lifecycleScope,
                        onNetworkStatusLog = { message ->
                            lifecycleScope.launch(Dispatchers.Main) { log(message) }
                        },
                    )
                val ref = system.start()

                serverSystem = system
                serverRef = ref

                withContext(Dispatchers.Main) {
                    statusText.text = "Status: Linked EchoService running"
                    stopButton.isEnabled = true
                    log("✅ EchoService started successfully")
                }
            } catch (e: Exception) {
                Log.e(TAG, "Failed to start linked EchoService", e)
                withContext(Dispatchers.Main) {
                    statusText.text = "Status: Start failed"
                    startButton.isEnabled = true
                    stopButton.isEnabled = false
                    log("❌ Start failed: ${e.message}")
                }
            }
        }
    }

    private fun stopServer() {
        stopButton.isEnabled = false
        log("Stopping EchoService server...")

        lifecycleScope.launch {
            try {
                serverRef?.stop()
                serverRef = null
                serverSystem?.close()
                serverSystem = null
            } catch (e: Exception) {
                Log.w(TAG, "Linked EchoService stop failed: ${e.message}")
            } finally {
                withContext(Dispatchers.Main) {
                    statusText.text = "Status: Stopped"
                    startButton.isEnabled = true
                    log("EchoService stopped")
                }
            }
        }
    }

    private fun copyAssetToInternalStorage(assetName: String): String {
        val inputStream = assets.open(assetName)
        val outputFile = File(filesDir, assetName)
        outputFile.parentFile?.mkdirs()
        inputStream.use { input ->
            outputFile.outputStream().use { output -> input.copyTo(output) }
        }
        return outputFile.absolutePath
    }

    private fun appendToLog(text: String) {
        // Auto-scroll only when user is at the bottom, avoiding forced layout spam
        val atBottom =
            scrollView.run {
                childCount > 0 && scrollY + height >= getChildAt(0).height - 20
            }
        logText.append(text)
        val excess = logText.length() - MAX_LOG_CHARS
        if (excess > 0) {
            logText.editableText.delete(0, excess)
        }
        if (atBottom) {
            scrollView.post { scrollView.fullScroll(ScrollView.FOCUS_DOWN) }
        }
    }

    private fun initLogcatReader() {
        logcatReader = LogcatReader { lines -> appendToLog(lines) }
        logcatReader.start()
    }

    private fun log(message: String) {
        Log.i(TAG, message)
        val currentTime =
            SimpleDateFormat("HH:mm:ss", Locale.getDefault())
                .format(Date())
        appendToLog("[$currentTime] $message\n")
    }

    private fun downloadLogs() {
        val text = logText.text.toString()
        if (text.isEmpty()) {
            Toast.makeText(this, "No logs to download", Toast.LENGTH_SHORT).show()
            return
        }
        try {
            val timestamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.getDefault()).format(Date())
            val fileName = "actr_logs_$timestamp.txt"
            val dir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
            dir.mkdirs()
            val file = File(dir, fileName)
            file.writeText(text)
            Toast.makeText(this, "Logs saved: ${file.absolutePath}", Toast.LENGTH_LONG).show()

            // Also offer to share
            val shareIntent =
                Intent(Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(Intent.EXTRA_TEXT, text)
                    putExtra(Intent.EXTRA_SUBJECT, "actr Logs $timestamp")
                }
            startActivity(Intent.createChooser(shareIntent, "Share Logs"))
        } catch (e: Exception) {
            Toast.makeText(this, "Failed to save logs: ${e.message}", Toast.LENGTH_LONG).show()
        }
    }

    /**
     * EchoService workload — implements both the [Workload] contract
     * (lifecycle methods) and the [EchoServiceHandler] contract (business logic).
     *
     * Workload and handler methods use the Kotlin DSL's application-facing aliases.
     */
    private class EchoServerWorkload :
        Workload,
        EchoServiceHandler {
        // -- Workload lifecycle --
        override suspend fun onStart(ctx: ActrContext) {
            Log.i(TAG, "EchoServerWorkload.onStart")
        }

        override suspend fun onReady(ctx: ActrContext) {
            Log.i(TAG, "EchoServerWorkload.onReady")
        }

        override suspend fun onStop(ctx: ActrContext) {
            Log.i(TAG, "EchoServerWorkload.onStop")
        }

        override suspend fun onError(
            ctx: ActrContext,
            event: ErrorEvent,
        ) {
            Log.e(TAG, "EchoServerWorkload.onError: $event")
        }

        override suspend fun dispatch(
            ctx: ActrContext,
            envelope: RpcEnvelope,
        ): ByteArray {
            if (envelope.routeKey != "echo.EchoService.Echo") {
                throw io.actrium.actr.ActrException.UnknownRoute(
                    "Unknown route key: ${envelope.routeKey}",
                )
            }
            val request = EchoRequest.parseFrom(envelope.payload)
            val response = echo(request, ctx)
            ctx.replyRaw(response.toByteArray())
            return ByteArray(0)
        }

        // -- Handler method --
        override suspend fun echo(
            request: EchoRequest,
            ctx: ActrContext,
        ): EchoResponse = EchoResponse.newBuilder().setReply("Echo: ${request.message}").build()
    }

    override fun onDestroy() {
        super.onDestroy()

        serverSystem?.cleanupConnections(CleanupReason.APP_TERMINATING)

        // Stop logcat reader
        if (::logcatReader.isInitialized) {
            logcatReader.stop()
        }

        serverRef?.shutdown()
        serverSystem?.close()
    }
}
