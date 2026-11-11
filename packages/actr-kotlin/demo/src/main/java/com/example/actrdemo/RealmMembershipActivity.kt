package com.example.actrdemo

import android.os.Bundle
import android.text.InputType
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import io.actrium.actr.RealmMembershipException
import io.actrium.actr.dsl.RealmIdentityStore
import io.actrium.actr.readRealmRuntimeConfiguration
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.net.URI

/** Development demo: endpoints come from actr.toml, provider is explicitly set.
 * The invite stays in a password field, never assets, preferences or logs. */
class RealmMembershipActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        val file = File(filesDir, "membership-runtime.toml")
        assets.open("actr.toml").use { input -> file.outputStream().use { input.copyTo(it) } }
        val config = readRealmRuntimeConfiguration(file.path)
        val store = RealmIdentityStore(this)
        val preferences = getSharedPreferences("realm-membership", MODE_PRIVATE)
        val layout =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(24, 24, 24, 24)
            }
        setContentView(ScrollView(this).apply { addView(layout) })
        layout.addView(TextView(this).apply { text = "Realm: ${config.realm}\nIssuer: ${config.issuer}" })
        val provider =
            EditText(this).apply {
                hint = "Trusted provider ID"
                setText(preferences.getString("provider:${config.realm}", URI(config.issuer).authority))
            }
        val invite =
            EditText(this).apply {
                hint = "One-time Realm invitation"
                inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                isSaveEnabled = false
                importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            }
        val target = EditText(this).apply { hint = "Client key ID to revoke (empty = this device)" }
        val status = TextView(this)
        layout.addView(provider)
        layout.addView(invite)
        layout.addView(target)
        layout.addView(status)
        var busy = false

        fun action(
            label: String,
            work: suspend () -> String,
        ) {
            layout.addView(
                Button(this).apply {
                    text = label
                    setOnClickListener {
                        if (busy) return@setOnClickListener
                        busy = true
                        val button = this
                        button.isEnabled = false
                        lifecycleScope.launch {
                            try {
                                status.text = withContext(Dispatchers.IO) { work() }
                            } catch (
                                error: RealmMembershipException.Rejected,
                            ) {
                                status.text = error.code
                            } catch (_: RealmMembershipException.Uncertain) {
                                status.text =
                                    "Result unknown. Retry the same invitation; your key is saved."
                            } catch (
                                _: Exception,
                            ) {
                                status.text = "Identity or membership unavailable. No key was replaced."
                            } finally {
                                busy = false
                                button.isEnabled = true
                            }
                        }
                    }
                },
            )
        }

        suspend fun membership() =
            store.membership(
                config.realm,
                withContext(Dispatchers.Main) {
                    provider.text.toString().trim()
                },
                config.issuer,
                true,
            )
        action("Join / retry invitation") {
            val providerId = withContext(Dispatchers.Main) { provider.text.toString().trim() }
            val text = withContext(Dispatchers.Main) { invite.text.toString().trim() }
            preferences.edit().putString("provider:${config.realm}", providerId).commit()
            val member = store.membership(config.realm, providerId, config.issuer, true)
            member.join(text, android.os.Build.MODEL)
            withContext(Dispatchers.Main) { invite.text.clear() }
            "Joined. Client key: ${member.keyId()}"
        }
        action("Restore / list devices") { membership().members() }
        action("Invite another device (15 minutes)") {
            val member = membership()
            val members = org.json.JSONObject(member.members()).getJSONArray("members")
            var memberId: String? = null
            for (i in 0 until members.length()) {
                val row = members.getJSONObject(i)
                val clients = row.getJSONArray("clients")
                for (j in 0 until clients.length()) {
                    if (clients.getJSONObject(j).getString("key_id") == member.keyId()) memberId = row.getString("member_id")
                }
            }
            val result =
                org.json.JSONObject(
                    member.inviteClient(
                        checkNotNull(memberId),
                        java.time.Instant
                            .now()
                            .plusSeconds(900)
                            .toString(),
                    ),
                )
            // FLAG_SECURE prevents screenshots; activity state is not persisted.
            result.getString("invite")
        }
        action("Revoke client") {
            val member = membership()
            val key = withContext(Dispatchers.Main) { target.text.toString().trim() }
            member.revokeClient(key.ifEmpty { member.keyId() })
            "Client revoked"
        }
        layout.addView(
            Button(this).apply {
                text = "Start a new identity after revocation"
                setOnClickListener {
                    if (busy) return@setOnClickListener
                    AlertDialog
                        .Builder(this@RealmMembershipActivity)
                        .setMessage(
                            "Discard this device's saved Realm identity? " +
                                "A new invitation is required. Other devices are unaffected.",
                        ).setNegativeButton("Cancel", null)
                        .setPositiveButton("Reset identity") { _, _ ->
                            status.text =
                                runCatching {
                                    store.forgetForRejoin(config.realm)
                                    "Enter a new invitation to rejoin."
                                }.getOrDefault("Unable to reset local identity.")
                        }.show()
                }
            },
        )
    }
}
