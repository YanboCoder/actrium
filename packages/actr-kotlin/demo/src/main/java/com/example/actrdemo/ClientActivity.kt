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
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import com.example.MyUnifiedHandler
import com.example.UnifiedLifecycleAdapter
import com.example.UnifiedWorkload
import echo.Echo.EchoRequest
import echo.Echo.EchoResponse
import io.actrium.actr.ActrException
import io.actrium.actr.ActrType
import io.actrium.actr.CleanupReason
import io.actrium.actr.PayloadType
import io.actrium.actr.dsl.*
import io.actrium.demo.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import local.StreamClientOuterClass.ClientStartStreamRequest
import local.StreamClientOuterClass.ClientStartStreamResponse
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

class ClientActivity : AppCompatActivity() {
    companion object {
        private const val TAG = "ClientActivity"

        // Limit log buffer to avoid exceeding Android clipboard ~1MB transaction limit
        private const val MAX_LOG_CHARS = 500_000
        private const val TAB_MAIN = 0
        private const val TAB_LOGS = 1
    }

    // Tab views
    private lateinit var mainTabButton: Button
    private lateinit var logsTabButton: Button
    private lateinit var mainTabContent: LinearLayout
    private lateinit var logsTabContent: LinearLayout

    // Main tab views
    private lateinit var statusText: TextView
    private lateinit var connectButton: Button
    private lateinit var disconnectButton: Button
    private lateinit var messageInput: EditText
    private lateinit var sendButton: Button
    private lateinit var sendFileButton: Button
    private lateinit var messageLogText: TextView
    private lateinit var messageScrollView: ScrollView

    // Logs tab views
    private lateinit var logText: TextView
    private lateinit var scrollView: ScrollView
    private lateinit var copyLogButton: Button
    private lateinit var downloadLogButton: Button
    private lateinit var clearLogButton: Button

    private var clientRef: ActrRef? = null
    private var clientSystem: ActrNode? = null

    // Tracks signaling + per-peer WebRTC state for display only. The framework
    // owns readiness waiting and delivery retry decisions.
    private var tracker: ConnectionTracker? = null

    // Logcat reader - streams native actr library logs to the UI
    private lateinit var logcatReader: LogcatReader

    /** Resolve the actor's ActrType from manifest.toml using the FFI-backed [Manifest] API. */
    private suspend fun resolveActorType(manifestPath: String): ActrType {
        val manifest = Manifest.from(java.io.File(manifestPath))
        return manifest.packageType()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_client)

        // Optional: forward actr runtime tracing logs to Android logcat.
        // Must be called before any ActrNode is created.
        setLogCallback(
            object : LogCallback {
                override fun onLog(
                    level: String,
                    target: String,
                    message: String,
                    timestampMs: Long,
                ) {
                    Log.println(
                        when (level.uppercase()) {
                            "ERROR" -> Log.ERROR
                            "WARN" -> Log.WARN
                            "INFO" -> Log.INFO
                            "DEBUG" -> Log.DEBUG
                            else -> Log.VERBOSE
                        },
                        "actr/$target",
                        message,
                    )
                }
            },
        )

        initViews()
        initLogcatReader() // Start early to capture actr library's early logs
        setupClickListeners()
        mainTabContent.addView(
            Button(this).apply {
                text = "Realm membership / devices"
                setOnClickListener {
                    if (clientSystem == null &&
                        connectButton.isEnabled
                    ) {
                        startActivity(Intent(this@ClientActivity, RealmMembershipActivity::class.java))
                    } else {
                        Toast.makeText(this@ClientActivity, "Disconnect before managing membership", Toast.LENGTH_SHORT).show()
                    }
                }
            },
            0,
        )
        switchToTab(TAB_MAIN)

        log("Ready to connect (linked multi-service workload)")
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

    private fun appendToMessageLog(text: String) {
        val atBottom =
            messageScrollView.run {
                childCount > 0 && scrollY + height >= getChildAt(0).height - 20
            }
        messageLogText.append(text)
        val excess = messageLogText.length() - MAX_LOG_CHARS
        if (excess > 0) {
            messageLogText.editableText.delete(0, excess)
        }
        if (atBottom) {
            messageScrollView.post { messageScrollView.fullScroll(ScrollView.FOCUS_DOWN) }
        }
    }

    private fun initLogcatReader() {
        logcatReader = LogcatReader { lines -> appendToLog(lines) }
        logcatReader.start()
    }

    private fun initViews() {
        // Tab views
        mainTabButton = findViewById(R.id.mainTabButton)
        logsTabButton = findViewById(R.id.logsTabButton)
        mainTabContent = findViewById(R.id.mainTabContent)
        logsTabContent = findViewById(R.id.logsTabContent)

        // Main tab views
        statusText = findViewById(R.id.statusText)
        connectButton = findViewById(R.id.connectButton)
        disconnectButton = findViewById(R.id.disconnectButton)
        messageInput = findViewById(R.id.messageInput)
        sendButton = findViewById(R.id.sendButton)
        sendFileButton = findViewById(R.id.sendFileButton)
        messageLogText = findViewById(R.id.messageLogText)
        messageScrollView = findViewById(R.id.messageScrollView)

        // Logs tab views
        logText = findViewById(R.id.logText)
        scrollView = findViewById(R.id.scrollView)
        copyLogButton = findViewById(R.id.copyLogButton)
        downloadLogButton = findViewById(R.id.downloadLogButton)
        clearLogButton = findViewById(R.id.clearLogButton)
    }

    private fun switchToTab(tab: Int) {
        val accentColor = Color.parseColor("#1976D2")
        val defaultColor = Color.parseColor("#E0E0E0")

        when (tab) {
            TAB_MAIN -> {
                mainTabContent.visibility = LinearLayout.VISIBLE
                logsTabContent.visibility = LinearLayout.GONE
                mainTabButton.backgroundTintList = ColorStateList.valueOf(accentColor)
                mainTabButton.setTextColor(Color.WHITE)
                logsTabButton.backgroundTintList = ColorStateList.valueOf(defaultColor)
                logsTabButton.setTextColor(Color.BLACK)
            }
            TAB_LOGS -> {
                mainTabContent.visibility = LinearLayout.GONE
                logsTabContent.visibility = LinearLayout.VISIBLE
                logsTabButton.backgroundTintList = ColorStateList.valueOf(accentColor)
                logsTabButton.setTextColor(Color.WHITE)
                mainTabButton.backgroundTintList = ColorStateList.valueOf(defaultColor)
                mainTabButton.setTextColor(Color.BLACK)
            }
        }
    }

    private fun setupClickListeners() {
        mainTabButton.setOnClickListener { switchToTab(TAB_MAIN) }
        logsTabButton.setOnClickListener { switchToTab(TAB_LOGS) }

        connectButton.setOnClickListener { connect() }
        disconnectButton.setOnClickListener { disconnect() }
        sendButton.setOnClickListener { sendMessage() }
        sendFileButton.setOnClickListener {
            val networkStatus = clientSystem?.getCurrentNetworkStatus() ?: "Network monitor not ready"
            log("📡 Current network: $networkStatus")
            clientSystem?.triggerNetworkCheck()
            sendFile()
        }

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

    private fun connect() {
        updateStatus("Connecting...")
        connectButton.isEnabled = false

        lifecycleScope.launch {
            try {
                val configPath = copyAssetToInternalStorage("actr.toml")
                val manifestPath = copyAssetToInternalStorage("manifest.toml")
                val realmConfig = io.actrium.actr.readRealmRuntimeConfiguration(configPath)
                val provider =
                    getSharedPreferences("realm-membership", MODE_PRIVATE)
                        .getString("provider:${realmConfig.realm}", null)
                        ?: error("Join this Realm using the membership screen first")
                val membership =
                    RealmIdentityStore(this@ClientActivity).membership(
                        realmConfig.realm,
                        provider,
                        realmConfig.issuer,
                        true,
                    )
                membership.members()
                Log.i(TAG, "Config path: $configPath")
                Log.i(TAG, "Manifest path: $manifestPath")

                val actorType = resolveActorType(manifestPath)
                Log.i(TAG, "Actor type from manifest: ${actorType.publisher}/${actorType.name}@${actorType.version}")

                // Host-side transport hooks: signaling + per-peer WebRTC state
                // drive status text only. They are not a send/retry mechanism.
                val t =
                    ConnectionTracker().apply {
                        onEvent = { msg ->
                            lifecycleScope.launch(Dispatchers.Main) {
                                logMessage("🔌 $msg")
                                refreshUi()
                            }
                        }
                    }
                tracker = t

                val workload = UnifiedWorkload(MyUnifiedHandler())
                val lifecycle = UnifiedLifecycleAdapter(workload)
                val system =
                    linkedWithMonitoring(
                        configPath = manifestPath,
                        membership = membership,
                        actorType = actorType,
                        workload =
                            lifecycle.toDynamicWorkload(
                                signaling = t.signalingObserver,
                                webrtc = t.webRtcObserver,
                            ),
                        context = this@ClientActivity,
                        scope = lifecycleScope,
                        onNetworkStatusLog = { message ->
                            lifecycleScope.launch(Dispatchers.Main) { log(message) }
                        },
                    )
                clientSystem = system
                Log.i(TAG, "✅ ActrNode created with retained NetworkMonitor")

                Log.i(TAG, "🚀 Starting linked multi-service actor...")
                clientRef = system.start()
                Log.i(TAG, "✅ Client started: ${clientRef?.actorId()?.serialNumber}")

                delay(2000)

                withContext(Dispatchers.Main) {
                    disconnectButton.isEnabled = true
                    messageInput.isEnabled = true
                    sendFileButton.isEnabled = true
                    logMessage("✅ Connected (linked multi-service mode)")
                    logMessage("🆔 Client ID: ${clientRef?.actorId()?.serialNumber}")
                    refreshUi()
                }
            } catch (e: Exception) {
                Log.e(TAG, "Connection failed", e)
                withContext(Dispatchers.Main) {
                    updateStatus("Connection failed")
                    connectButton.isEnabled = true
                    log("Error: ${e.message}")
                }
            }
        }
    }

    private fun disconnect() {
        updateStatus("Disconnecting...")
        disconnectButton.isEnabled = false
        messageInput.isEnabled = false
        sendButton.isEnabled = false
        sendFileButton.isEnabled = false

        lifecycleScope.launch {
            try {
                clientRef?.stop()
                clientRef = null
                clientSystem?.close()
                clientSystem = null
                tracker?.reset()
                tracker = null

                withContext(Dispatchers.Main) {
                    updateStatus("Disconnected")
                    connectButton.isEnabled = true
                    logMessage("🔌 Disconnected")
                }
            } catch (e: Exception) {
                Log.e(TAG, "Disconnect error", e)
                withContext(Dispatchers.Main) {
                    updateStatus("Disconnected")
                    connectButton.isEnabled = true
                    clientRef = null
                    clientSystem?.close()
                    clientSystem = null
                    log("Disconnect error: ${e.message}")
                }
            }
        }
    }

    private fun sendMessage() {
        val message = messageInput.text.toString().trim()
        if (message.isEmpty()) return

        val ref = clientRef
        if (ref == null) {
            log("Error: Not connected")
            return
        }

        messageInput.text.clear()
        logMessage("📤 Sent: $message")

        lifecycleScope.launch {
            attemptEcho(ref, message)
        }
    }

    /** The framework waits for a usable route and owns transport-level retry. */
    private suspend fun attemptEcho(
        ref: ActrRef,
        message: String,
    ) {
        try {
            val request = EchoRequest.newBuilder().setMessage(message).build()
            val responsePayload =
                ref.call(
                    "echo.EchoService.Echo",
                    PayloadType.RPC_RELIABLE,
                    request.toByteArray(),
                    30000L,
                )
            val response = EchoResponse.parseFrom(responsePayload)
            Log.i(TAG, "📬 Echo Response: ${response.reply}")
            withContext(Dispatchers.Main) {
                logMessage("📥 Received: ${response.reply}")
                refreshUi()
            }
        } catch (e: ActrException.ConnectionNotReady) {
            Log.e(TAG, "Framework reported terminal connection-not-ready", e)
            withContext(Dispatchers.Main) { logMessage("❌ Echo route not ready: ${e.message}") }
        } catch (e: Exception) {
            Log.e(TAG, "Echo send error", e)
            withContext(Dispatchers.Main) { logMessage("❌ Echo error: ${e.message}") }
        }
    }

    private fun sendFile() {
        val ref = clientRef
        if (ref == null) {
            log("Error: Not connected")
            return
        }

        logMessage("📤 Starting stream transfer...")

        lifecycleScope.launch {
            try {
                val request =
                    ClientStartStreamRequest
                        .newBuilder()
                        .setClientId("android-client")
                        .setSessionId("session-${System.currentTimeMillis()}")
                        .setMessageCount(3)
                        .build()

                val responsePayload =
                    ref.call(
                        "data_stream_peer.StreamClient.StartStream",
                        PayloadType.RPC_RELIABLE,
                        request.toByteArray(),
                        60000L,
                    )

                val response = ClientStartStreamResponse.parseFrom(responsePayload)
                Log.i(
                    TAG,
                    "📬 StartStream Response: accepted=${response.accepted}, message=${response.message}",
                )

                withContext(Dispatchers.Main) {
                    if (response.accepted) {
                        logMessage("✅ Stream transfer completed")
                        logMessage("📝 ${response.message}")
                    } else {
                        logMessage("❌ Stream transfer rejected: ${response.message}")
                    }
                }
            } catch (e: Exception) {
                Log.e(TAG, "Stream transfer error", e)
                withContext(Dispatchers.Main) { logMessage("❌ Stream transfer error: ${e.message}") }
            }
        }
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

    private fun updateStatus(status: String) {
        statusText.text = "Status: $status"
    }

    /** Reflect observational signaling + per-peer WebRTC state in the status bar. */
    private fun refreshUi() {
        val t = tracker
        if (t == null) {
            sendButton.isEnabled = false
            return
        }
        val sig = if (t.signalingReady.get()) "✓" else "✗"
        val webrtc =
            t.webRtcStatus.entries
                .firstOrNull()
                ?.let { "${it.key.serialNumber}:${it.value.name.lowercase()}" }
                ?: "idle"
        statusText.text = "Status: signaling=$sig webrtc=$webrtc"
        // A recovering peer is still a valid user action: the framework waits
        // within the operation deadline instead of requiring a UI retry gate.
        sendButton.isEnabled = messageInput.isEnabled
    }

    private fun log(message: String) {
        Log.i(TAG, message)
        val currentTime =
            SimpleDateFormat("HH:mm:ss", Locale.getDefault())
                .format(Date())
        appendToLog("[$currentTime] $message\n")
    }

    /** Log a sent/received message — shows in the Main tab message log only. */
    private fun logMessage(message: String) {
        Log.i(TAG, message)
        val currentTime =
            SimpleDateFormat("HH:mm:ss", Locale.getDefault())
                .format(Date())
        val timestamped = "[$currentTime] $message\n"
        appendToMessageLog(timestamped)
    }

    override fun onResume() {
        super.onResume()
        clientSystem?.onAppForeground()
    }

    override fun onPause() {
        super.onPause()
        clientSystem?.onAppBackground()
    }

    override fun onDestroy() {
        super.onDestroy()

        clientSystem?.cleanupConnections(CleanupReason.APP_TERMINATING)

        // Stop logcat reader
        if (::logcatReader.isInitialized) {
            logcatReader.stop()
        }

        lifecycleScope.launch {
            try {
                clientSystem?.close()
                clientSystem = null
            } catch (e: Exception) {
                Log.w(TAG, "Error during onDestroy cleanup: ${e.message}")
            }
        }
    }
}
