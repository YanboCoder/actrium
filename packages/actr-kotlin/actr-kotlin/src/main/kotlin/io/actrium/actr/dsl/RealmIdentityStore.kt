package io.actrium.actr.dsl

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import io.actrium.actr.RealmClientSigner
import io.actrium.actr.RealmMembership
import io.actrium.actr.RealmMembershipException
import io.actrium.actr.realmKeyGenerateSeed
import io.actrium.actr.realmKeyPublic
import io.actrium.actr.realmKeySign
import java.io.File
import java.io.RandomAccessFile
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Per-install, per-Realm Ed25519 identity. Android Keystore holds a non-exportable
 * AES-GCM wrapping key; the encrypted Ed25519 seed lives only in noBackupFilesDir.
 * Software Ed25519 is intentional: not every supported device has secure Ed25519.
 * Corrupt/restored ciphertext fails closed and never silently generates a new key.
 */
class RealmIdentityStore(context: Context) {
    private val root = File(context.applicationContext.noBackupFilesDir, "actr-realm-identities")

    private fun <T> locked(block: (String) -> T): T = synchronized(processLock) {
        check(root.isDirectory || root.mkdirs()) { "Realm identity storage unavailable" }
        RandomAccessFile(File(root, "store.lock"), "rw").use { file ->
            file.channel.lock().use {
                val marker = AtomicFile(File(root, "installation"))
                val installation = if (exists(marker)) {
                    UUID.fromString(String(marker.readFully(), Charsets.UTF_8)).toString()
                } else {
                    UUID.randomUUID().toString().also { write(marker, it.toByteArray()) }
                }
                block(installation)
            }
        }
    }

    private fun name(realm: String): String =
        MessageDigest.getInstance("SHA-256").digest(realm.toByteArray()).joinToString("") { "%02x".format(it) }

    private fun alias(installation: String, realm: String) = "actr.realm.$installation.${name(realm)}"
    private fun record(realm: String) = AtomicFile(File(root, "${name(realm)}.key"))
    private fun aad(installation: String, realm: String) = "actr.realm-client/v1:$installation:$realm".toByteArray()
    private fun keyStore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun wrappingKey(installation: String, realm: String, create: Boolean): SecretKey {
        val alias = alias(installation, realm)
        val existing = keyStore().getKey(alias, null)
        if (existing != null) return existing as SecretKey
        check(create) { "Realm identity wrapping key unavailable" }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
            generateKey()
        }
    }

    private fun read(installation: String, realm: String): ByteArray {
        val bytes = record(realm).readFully()
        check(bytes.size == 61 && bytes[0] == 1.toByte()) { "Invalid Realm identity record" }
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, wrappingKey(installation, realm, false), GCMParameterSpec(128, bytes.copyOfRange(1, 13)))
            updateAAD(aad(installation, realm))
            doFinal(bytes.copyOfRange(13, bytes.size))
        }
    }

    private fun create(installation: String, realm: String) {
        val seed = realmKeyGenerateSeed()
        try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, wrappingKey(installation, realm, true))
            cipher.updateAAD(aad(installation, realm))
            write(record(realm), byteArrayOf(1) + cipher.iv + cipher.doFinal(seed))
        } finally { seed.fill(0) }
    }

    /** Call before joining. A retry or app restart always recovers the same key. */
    fun membership(realm: String, providerId: String, issuer: String, allowInsecureDevelopment: Boolean = false): RealmMembership {
        val publicKey = locked { installation ->
            if (!exists(record(realm))) {
                check(!keyStore().containsAlias(alias(installation, realm))) { "Realm identity ciphertext is missing; explicitly reset before rejoining" }
                create(installation, realm)
            }
            val seed = read(installation, realm)
            try { realmKeyPublic(seed) } finally { seed.fill(0) }
        }
        val signer = object : RealmClientSigner {
            override fun publicKey(): ByteArray = publicKey.copyOf()
            override fun sign(payload: ByteArray): ByteArray = try {
                locked { installation ->
                    val seed = read(installation, realm)
                    try {
                        check(realmKeyPublic(seed).contentEquals(publicKey)) { "Realm identity changed" }
                        realmKeySign(seed, payload)
                    } finally { seed.fill(0) }
                }
            } catch (_: Exception) { throw RealmMembershipException.KeyUnavailable() }
        }
        return RealmMembership(realm, providerId, issuer, signer, allowInsecureDevelopment)
    }

    /** Explicit rejoin only, after stopping Hosts and confirming loss/revocation.
     * Old signer handles become unusable. This does not revoke the server record. */
    fun forgetForRejoin(realm: String) = locked { installation ->
        record(realm).delete()
        keyStore().deleteEntry(alias(installation, realm))
    }

    companion object {
        private val processLock = Any()
        private fun exists(file: AtomicFile) = file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()
        private fun write(file: AtomicFile, bytes: ByteArray) {
            val stream = file.startWrite()
            try { stream.write(bytes); file.finishWrite(stream) }
            catch (error: Exception) { file.failWrite(stream); throw error }
        }
    }
}
