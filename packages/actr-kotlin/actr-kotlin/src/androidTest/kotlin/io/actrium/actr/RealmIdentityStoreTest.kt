package io.actrium.actr

import android.content.Context
import android.content.ContextWrapper
import androidx.test.platform.app.InstrumentationRegistry
import io.actrium.actr.dsl.RealmIdentityStore
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.Assume.assumeTrue
import java.io.File
import java.security.KeyStore
import java.util.UUID

private class RenewalObserver : CredentialObserverBridge {
    val renewals = java.util.concurrent.atomic.AtomicInteger()
    override suspend fun onRenewed(ctx: ContextBridge, event: CredentialEventBridge) { renewals.incrementAndGet() }
    override suspend fun onExpiring(ctx: ContextBridge, event: CredentialEventBridge) { }
}

class RealmIdentityStoreTest {
    private fun sandbox(context: Context, root: File) = object : ContextWrapper(context) {
        override fun getApplicationContext(): Context = this
        override fun getNoBackupFilesDir(): File = root
    }

    @Test fun realIssuerJoinHostRegisterAndRevoke(): Unit = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixtureFile = File(context.noBackupFilesDir, "issuer-fixture.json")
        assumeTrue("Run the mobile Issuer fixture harness", fixtureFile.exists())
        val fixture = org.json.JSONObject(fixtureFile.readText())
        fixtureFile.delete()
        val root = File(context.noBackupFilesDir, "issuer-${UUID.randomUUID()}")
        val a = RealmIdentityStore(sandbox(context, File(root, "a")))
        val b = RealmIdentityStore(sandbox(context, File(root, "b")))
        val realm = fixture.getString("realm")
        val issuer = fixture.getString("issuer").replace("127.0.0.1", "10.0.2.2")
        val provider = fixture.getString("provider_id")
        val signaling = fixture.getString("signaling").replace("127.0.0.1", "10.0.2.2")
        var refA: io.actrium.actr.dsl.ActrRef? = null
        var refB: io.actrium.actr.dsl.ActrRef? = null
        val ma = a.membership(realm, provider, issuer, true)
        val mb = b.membership(realm, provider, issuer, true)
        var phase = "join first device"
        try {
            val joined = org.json.JSONObject(ma.join(fixture.getString("invite"), "Android A"))
            phase = "repeat join"
            assertEquals(ma.keyId(), org.json.JSONObject(ma.join(fixture.getString("invite"), "retry")).getString("key_id"))
            phase = "invite second device"
            val invitation = org.json.JSONObject(ma.inviteClient(joined.getString("member_id"), java.time.Instant.now().plusSeconds(900).toString()))
            phase = "join second device"
            mb.join(invitation.getString("invite"), "Android B")
            assertNotEquals(ma.keyId(), mb.keyId())
            suspend fun register(membership: RealmMembership, observer: CredentialObserverBridge? = null): io.actrium.actr.dsl.ActrNode {
                val config = File(root, "runtime.toml")
                config.writeText("""
                    [deployment]
                    realm = "$realm"
                    [issuer_endpoint]
                    url = "$issuer"
                    [signaling]
                    url = "$signaling"
                    [hyper]
                    data_dir = "${File(root, membership.keyId()).path}"
                    [hyper.trust]
                    kind = "dev_only"
                """.trimIndent())
                return io.actrium.actr.dsl.linked(config.path, ActrType("sdk", "device", "1.0.0"),
                    io.actrium.actr.dsl.dynamicWorkload(io.actrium.actr.dsl.SimpleWorkload("sdk/device@1.0.0"), credential = observer), membership)
            }
            phase = "register hosts"
            val observerA = RenewalObserver()
            val observerB = RenewalObserver()
            refA = register(ma, observerA).start()
            refB = register(mb, observerB).start()
            phase = "automatic credential renewal"
            kotlinx.coroutines.withTimeout(60000) {
                while (observerA.renewals.get() < 2 || observerB.renewals.get() < 2) kotlinx.coroutines.delay(100)
            }
            val bBeforeRevoke = observerB.renewals.get()
            phase = "revoke first device"
            mb.revokeClient(ma.keyId())
            try { ma.members(); fail("revoked client must be refused") }
            catch (error: RealmMembershipException.Rejected) { assertEquals("client_revoked", error.code) }
            try { register(ma).close(); fail("revoked client cannot register") }
            catch (_: ActrException) { }
            register(mb).close()
            mb.members()
            kotlinx.coroutines.withTimeout(60000) {
                while (observerB.renewals.get() <= bBeforeRevoke) kotlinx.coroutines.delay(100)
            }
            // Let the revoked A attempt its next real Issuer renewal. The
            // harness asserts the Issuer's rejection counters independently.
            kotlinx.coroutines.delay(10000)
        } catch (error: Exception) {
            throw AssertionError("Failed membership phase: $phase", error)
        } finally {
            refA?.stop(); refB?.stop()
            a.forgetForRejoin(realm); b.forgetForRejoin(realm); root.deleteRecursively()
        }
    }

    @Test fun installationRealmRestartResetAndBackupIsolation() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.noBackupFilesDir, "test-${UUID.randomUUID()}")
        val aContext = sandbox(context, File(root, "a"))
        val bContext = sandbox(context, File(root, "b"))
        val a = RealmIdentityStore(aContext)
        val b = RealmIdentityStore(bContext)
        fun member(store: RealmIdentityStore, realm: String = "one@example.com") =
            store.membership(realm, "provider.example", "https://issuer.example")
        try {
            val old = member(a)
            assertEquals(old.keyId(), member(RealmIdentityStore(aContext)).keyId())
            assertNotEquals(old.keyId(), member(b).keyId())
            assertNotEquals(old.keyId(), member(a, "two@example.com").keyId())
            a.forgetForRejoin("one@example.com")
            assertNotEquals(old.keyId(), member(a).keyId())
            try { old.members(); fail("old signer must not sign with new key") }
            catch (_: RealmMembershipException.KeyUnavailable) {}
            // Restoring encrypted files without this installation's Keystore key
            // cannot clone a usable identity or silently rotate it.
            val installation = File(aContext.noBackupFilesDir, "actr-realm-identities/installation").readText()
            val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            keyStore.aliases().toList().filter { it.startsWith("actr.realm.$installation.") }.forEach(keyStore::deleteEntry)
            try { member(a); fail("missing wrapping key must fail closed") }
            catch (_: IllegalStateException) {}
        } finally {
            runCatching { a.forgetForRejoin("one@example.com") }
            runCatching { a.forgetForRejoin("two@example.com") }
            runCatching { b.forgetForRejoin("one@example.com") }
            root.deleteRecursively()
        }
    }
}
